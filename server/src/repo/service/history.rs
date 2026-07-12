use serde::Serialize;
use std::collections::{HashMap, HashSet};

use crate::error::AppError;
use crate::repository::Repositories;
use crate::serialization::{S_IFDIR, S_IFREG};
use base::common::{FsDirData, FsFileData};

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
        let _new_dirs: Vec<DirChange> = Vec::new();
        let _deleted_dirs: Vec<DirChange> = Vec::new();

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
