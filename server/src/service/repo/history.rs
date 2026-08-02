use sea_orm::DatabaseConnection;
use serde::Serialize;
use std::collections::{HashMap, HashSet};

use crate::fs::core::file_ops::FileOps;
use crate::fs::core::{read_fs_file_data, resolve_fs_id};
use crate::repository::Repositories;
use base::common::{FsDirData, FsFileData};
use base::error::AppError;
use infra::activity_log;
use infra::common::util::{get_head_root_id, parent_path_from};
use infra::serialization::{S_IFDIR, S_IFREG};

#[derive(Serialize)]
pub struct HistoryChangesResponse {
    pub new_files: Vec<FileChange>,
    pub deleted_files: Vec<FileChange>,
    pub modified_files: Vec<FileChange>,
    pub renamed_files: Vec<FileChange>,
    pub new_dirs: Vec<DirChange>,
    pub deleted_dirs: Vec<DirChange>,
}

#[derive(Serialize)]
pub struct FileChange {
    pub path: String,
    pub size: i64,
}

#[derive(Serialize)]
pub struct DirChange {
    pub path: String,
}

/// Walk an FS tree and collect all file paths with their size info.
async fn collect_files(
    repos: &Repositories,
    repo_id: &str,
    root_id: &str,
    prefix: &str,
    files: &mut HashMap<String, i64>,
    visited: &mut HashSet<String>,
) -> Result<(), AppError> {
    // EMPTY_SHA1 is the sentinel for empty directories — no fs_object record.
    if root_id == "0000000000000000000000000000000000000000" {
        return Ok(());
    }
    if !visited.insert(root_id.to_string()) {
        return Ok(()); // Already visited this dir
    }

    let obj = repos
        .fs_object
        .find_by_repo_and_fs_id(repo_id, root_id)
        .await?
        .ok_or_else(|| AppError::NotFound("fs object not found".into()))?;

    if obj.obj_type == 1i8 {
        // File
        let file_data: FsFileData =
            serde_json::from_str(&obj.data).map_err(|e| AppError::Internal(e.to_string()))?;
        files.insert(prefix.to_string(), file_data.size);
    } else if obj.obj_type == 3i8 {
        // Directory
        let dir_data: FsDirData =
            serde_json::from_str(&obj.data).map_err(|e| AppError::Internal(e.to_string()))?;

        for entry in &dir_data.dirents {
            let child_path = if prefix == "/" {
                format!("/{}", entry.name)
            } else {
                format!("{}/{}", prefix, entry.name)
            };

            if entry.mode == S_IFREG || entry.size > 0 {
                // File entry
                files.insert(child_path.clone(), entry.size);
            } else if entry.mode == S_IFDIR {
                // Directory entry - recurse
                Box::pin(collect_files(
                    repos,
                    repo_id,
                    &entry.id,
                    &child_path,
                    files,
                    visited,
                ))
                .await?;
            } else {
                // Skip — could be a symlink or other type
            }
        }
    }

    Ok(())
}

/// Service for repo history-related operations.
pub struct HistoryService;

impl HistoryService {
    /// Returns the file changes introduced by a specific commit.
    ///
    /// This compares the FS objects of the commit's root directory against
    /// those of its parent commit (or returns all files for the initial commit).
    pub async fn get_history_changes(
        repos: &Repositories,
        repo_id: &str,
        commit_id: &str,
    ) -> Result<HistoryChangesResponse, AppError> {
        // Find the commit
        let c = repos
            .commit
            .find_by_repo_and_commit_id(repo_id, commit_id)
            .await?
            .ok_or_else(|| AppError::NotFound("commit not found".into()))?;

        let mut new_files = Vec::new();
        let mut deleted_files = Vec::new();
        let mut modified_files = Vec::new();
        let renamed_files = Vec::new();

        // Collect files from the current commit
        let mut current_files: HashMap<String, i64> = HashMap::new();
        Box::pin(collect_files(
            repos,
            repo_id,
            &c.root_id,
            "/",
            &mut current_files,
            &mut HashSet::new(),
        ))
        .await?;

        if let Some(parent_id) = &c.parent_id {
            // Find parent commit to get its root_id
            let parent_commit = repos
                .commit
                .find_by_repo_and_commit_id(repo_id, parent_id)
                .await?
                .ok_or_else(|| AppError::NotFound("parent commit not found".into()))?;

            let mut parent_files: HashMap<String, i64> = HashMap::new();
            Box::pin(collect_files(
                repos,
                repo_id,
                &parent_commit.root_id,
                "/",
                &mut parent_files,
                &mut HashSet::new(),
            ))
            .await?;

            // Compare to find changes
            for (path, size) in &current_files {
                match parent_files.get(path) {
                    None => {
                        new_files.push(FileChange {
                            path: path.clone(),
                            size: *size,
                        });
                    }
                    Some(old_size) if old_size != size => {
                        modified_files.push(FileChange {
                            path: path.clone(),
                            size: *size,
                        });
                    }
                    _ => {}
                }
            }

            for (path, size) in &parent_files {
                if !current_files.contains_key(path) {
                    deleted_files.push(FileChange {
                        path: path.clone(),
                        size: *size,
                    });
                }
            }
        } else {
            // Initial commit — all files are "new"
            for (path, size) in &current_files {
                new_files.push(FileChange {
                    path: path.clone(),
                    size: *size,
                });
            }
        }

        Ok(HistoryChangesResponse {
            new_files,
            deleted_files,
            modified_files,
            renamed_files,
            new_dirs: Vec::new(),
            deleted_dirs: Vec::new(),
        })
    }
}

/// A single version of a file, in the seahub `FileHistoryItem` field layout.
#[derive(Serialize)]
pub struct FileHistoryItem {
    pub commit_id: String,
    pub path: String,
    pub size: i64,
    pub mtime: i64,
    pub last_modified_by: String,
    pub rev_file_id: String,
    pub rev_file_name: String,
    pub rev_desc: String,
    pub file_name: String,
    pub file_size: i64,
    pub file_mtime: i64,
    pub file_type: String,
}

impl HistoryService {
    /// Return the version history of a single file, newest first.
    ///
    /// Walks the repo's commits newest→oldest and records each commit whose
    /// file content (`fs_id`) differs from the previously recorded one; stops
    /// as soon as the path no longer resolves (the file's creation commit).
    pub async fn get_file_history(
        repos: &Repositories,
        repo_id: &str,
        path: &str,
        limit: u64,
    ) -> Result<Vec<FileHistoryItem>, AppError> {
        let commits = repos
            .commit
            .find_by_repo_id_ordered_by_ctime_desc(repo_id)
            .await?;

        let file_name = path
            .rsplit_once('/')
            .map(|(_, n)| n)
            .unwrap_or(path)
            .to_string();

        let mut result = Vec::new();
        let mut last_fs_id: Option<String> = None;
        for c in &commits {
            if result.len() >= limit as usize {
                break;
            }
            let fs_id = match resolve_fs_id(repos, repo_id, &c.root_id, path).await {
                Ok(id) => id,
                Err(_) => break, // path doesn't exist in this commit or earlier ones
            };
            if last_fs_id.as_deref() == Some(fs_id.as_str()) {
                continue; // content unchanged in this commit
            }
            let size = read_fs_file_data(repos, repo_id, &fs_id)
                .await
                .map(|f| f.size)
                .unwrap_or(0);
            result.push(FileHistoryItem {
                commit_id: c.commit_id.clone(),
                path: path.to_string(),
                size,
                mtime: c.ctime,
                last_modified_by: c.creator_name.clone(),
                rev_file_id: fs_id.clone(),
                rev_file_name: file_name.clone(),
                rev_desc: c.description.clone(),
                file_name: file_name.clone(),
                file_size: size,
                file_mtime: c.ctime,
                file_type: "file".to_string(),
            });
            last_fs_id = Some(fs_id);
        }
        Ok(result)
    }

    /// Resolve a file's `(fs_id, FsFileData)` at a specific historical commit.
    pub async fn get_file_revision(
        repos: &Repositories,
        repo_id: &str,
        commit_id: &str,
        path: &str,
    ) -> Result<(String, FsFileData), AppError> {
        let c = repos
            .commit
            .find_by_repo_and_commit_id(repo_id, commit_id)
            .await?
            .ok_or_else(|| AppError::NotFound("commit not found".into()))?;

        let fs_id = resolve_fs_id(repos, repo_id, &c.root_id, path)
            .await
            .map_err(|_| AppError::NotFound("file not found in this version".into()))?;
        let file_data = read_fs_file_data(repos, repo_id, &fs_id).await?;
        Ok((fs_id, file_data))
    }

    /// Restore a file to one of its historical versions by pointing the file's
    /// dirent at the target commit's `fs_id` and committing the change.
    pub async fn restore_file_revision(
        db: &DatabaseConnection,
        repos: &Repositories,
        repo_id: &str,
        commit_id: &str,
        path: &str,
        modifier: &str,
        user_id: i32,
    ) -> Result<(), AppError> {
        let c = repos
            .commit
            .find_by_repo_and_commit_id(repo_id, commit_id)
            .await?
            .ok_or_else(|| AppError::NotFound("commit not found".into()))?;

        let target_fs_id = resolve_fs_id(repos, repo_id, &c.root_id, path)
            .await
            .map_err(|_| AppError::NotFound("file not found in target version".into()))?;
        // Fails with NotFound if the old fs_object was already garbage-collected.
        let target_file = read_fs_file_data(repos, repo_id, &target_fs_id).await?;

        let name = path
            .rsplit_once('/')
            .map(|(_, n)| n)
            .unwrap_or(path)
            .to_string();
        let parent_path = parent_path_from(path);
        let head_root_id = get_head_root_id(db, repo_id).await?;
        let parent_fs_id = resolve_fs_id(repos, repo_id, &head_root_id, parent_path).await?;

        let now = chrono::Utc::now().timestamp();
        let target_size = target_file.size;

        FileOps::update_dir_tree_and_commit(
            db,
            repos,
            repo_id,
            parent_path,
            &parent_fs_id,
            modifier,
            &format!("Reverted {name} to version from {commit_id}"),
            crate::fs::core::file_ops::EMPTY_ANCESTOR_CHAIN,
            |dirents| {
                if let Some(d) = dirents.iter_mut().find(|d| d.name == name) {
                    d.id = target_fs_id.clone();
                    d.size = target_size;
                    d.mtime = now;
                    d.modifier = modifier.to_string();
                }
                Ok(())
            },
        )
        .await?;

        activity_log::log_activity(
            db,
            repo_id,
            "recover",
            "file",
            path,
            user_id,
            None,
            Some(target_size),
            Some(&target_fs_id),
            None,
            None,
        )
        .await;

        Ok(())
    }
}
