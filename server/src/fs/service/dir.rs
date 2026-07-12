use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use sea_orm::{ConnectionTrait, DatabaseConnection};

use crate::activity_log;
use crate::common::DirEntry;
use crate::common::util::{get_head_commit_id, get_head_root_id, parent_path_from};
use crate::entity::{repo, repo_member};
use crate::error::AppError;
use crate::repo::file_ops::FileOps;
use crate::repository::Repositories;
use crate::serialization::S_IFDIR;
use crate::serialization::fs_json::{DirEntryData, FsDirData, SEAF_METADATA_TYPE_DIR};

// ── Free-standing pub(crate) helpers (used by src/ui/files.rs) ──────────

/// List directory entries by traversing the FS object tree from the head commit.
pub(crate) async fn list_dir_from_fs_tree(
    _db: &DatabaseConnection,
    repos: &Repositories,
    repo_id: &str,
    path: &str,
) -> Result<(String, Vec<DirEntry>), AppError> {
    let repo_record = repos
        .repo
        .find_by_id(repo_id)
        .await?
        .ok_or_else(|| AppError::NotFound("Repository not found".into()))?;

    let head_commit_id = match repo_record.head_commit_id {
        Some(id) => id,
        None => return Ok((String::new(), vec![])),
    };

    let head = repos
        .commit
        .find_by_repo_and_commit_id(repo_id, &head_commit_id)
        .await?
        .ok_or_else(|| AppError::NotFound("Head commit not found".into()))?;

    let dir_id = match crate::repo::resolve_fs_id(repos, repo_id, &head.root_id, path).await {
        Ok(id) => id,
        Err(e) => {
            let msg = e.to_string();
            if msg.starts_with("path segment not found") {
                return Err(AppError::NotFound(msg.to_string()));
            }
            return Err(AppError::internal(format!("resolve_fs_id failed: {e}")));
        }
    };

    let dir_data = read_fs_dir_data(repos, repo_id, &dir_id)
        .await
        .map_err(|e| AppError::NotFound(format!("not a directory: {e}")))?;

    Ok((
        dir_id,
        dir_data
            .dirents
            .into_iter()
            .map(|d| DirEntry {
                id: d.id,
                entry_type: if d.mode & S_IFDIR != 0 {
                    "dir".to_string()
                } else {
                    "file".to_string()
                },
                name: d.name,
                size: d.size,
                mtime: d.mtime,
                permission: "rw".to_string(),
                modifier: d.modifier,
                parent_dir: None,
                modifier_name: None,
                modifier_contact_email: None,
            })
            .collect(),
    ))
}

/// Recursively list all directory entries from the FS object tree.
pub(crate) async fn list_dir_recursive_from_fs_tree(
    _db: &DatabaseConnection,
    repos: &Repositories,
    repo_id: &str,
    path: &str,
) -> Result<(String, Vec<DirEntry>), AppError> {
    let repo_record = repos
        .repo
        .find_by_id(repo_id)
        .await?
        .ok_or_else(|| AppError::NotFound("Repository not found".into()))?;

    let head_commit_id = match repo_record.head_commit_id {
        Some(id) => id,
        None => return Ok((String::new(), vec![])),
    };

    let head = repos
        .commit
        .find_by_repo_and_commit_id(repo_id, &head_commit_id)
        .await?
        .ok_or_else(|| AppError::NotFound("Head commit not found".into()))?;

    let dir_id = match crate::repo::resolve_fs_id(repos, repo_id, &head.root_id, path).await {
        Ok(id) => id,
        Err(e) => {
            let msg = e.to_string();
            if msg.starts_with("path segment not found") {
                return Err(AppError::NotFound(msg.to_string()));
            }
            return Err(AppError::internal(format!("resolve_fs_id failed: {e}")));
        }
    };

    let mut stack: Vec<(String, String)> = vec![(dir_id.clone(), path.to_string())];
    let mut entries: Vec<DirEntry> = Vec::new();

    while let Some((fs_id, parent_path)) = stack.pop() {
        if fs_id == "0000000000000000000000000000000000000000" {
            continue;
        }

        let dir_data = match crate::repo::read_fs_dir_data(repos, repo_id, &fs_id).await {
            Ok(d) => d,
            Err(_) => continue,
        };

        for dirent in &dir_data.dirents {
            let is_dir = dirent.mode & S_IFDIR != 0;
            let modifier_email = dirent.modifier.clone();

            let mut entry = DirEntry {
                id: dirent.id.clone(),
                entry_type: if is_dir {
                    "dir".to_string()
                } else {
                    "file".to_string()
                },
                name: dirent.name.clone(),
                size: dirent.size,
                mtime: dirent.mtime,
                permission: "rw".to_string(),
                modifier: modifier_email.clone(),
                parent_dir: Some(parent_path.clone()),
                modifier_name: None,
                modifier_contact_email: None,
            };

            if !is_dir && !modifier_email.is_empty() {
                let modifier_name = repos
                    .user
                    .find_by_email(&modifier_email)
                    .await?
                    .map(|u| u.nickname())
                    .unwrap_or_else(|| modifier_email.split('@').next().unwrap_or("").to_string());
                entry.modifier_name = Some(modifier_name);
                entry.modifier_contact_email = Some(modifier_email);
            }

            entries.push(entry);

            if is_dir {
                let child_path = if parent_path == "/" {
                    format!("/{}", dirent.name)
                } else {
                    format!("{}/{}", parent_path, dirent.name)
                };
                stack.push((dirent.id.clone(), child_path));
            }
        }
    }

    Ok((dir_id, entries))
}

/// Create a directory at the given path.
pub(crate) async fn create_dir_by_path(
    db: &DatabaseConnection,
    repos: &Repositories,
    email: &str,
    user_id: i32,
    repo_id: &str,
    path: &str,
) -> Result<(), AppError> {
    // Validate and canonicalize the path to prevent directory traversal attacks
    let path = crate::sanitize::canonicalize_path(path).map_err(|e| {
        AppError::BadRequest(format!(
            "Invalid directory path: {}. Please ensure the path does not contain '..' components that would escape the repository.",
            e
        ))
    })?;

    let parts: Vec<&str> = path.trim_start_matches('/').split('/').collect();
    if parts.is_empty() {
        return Err(AppError::BadRequest("invalid path".into()));
    }

    let dir_name = parts.last().unwrap();
    let parent_path = if parts.len() > 1 {
        format!("/{}", parts[..parts.len() - 1].join("/"))
    } else {
        "/".to_string()
    };

    let dir_fs_id = "0000000000000000000000000000000000000000".to_string();
    let now = chrono::Utc::now().timestamp();

    let parent_fs_id = if parent_path == "/" {
        match get_head_root_id(db, repo_id).await {
            Ok(root_id) => root_id,
            Err(_) => {
                let empty_root = FsDirData {
                    dirents: vec![],
                    obj_type: SEAF_METADATA_TYPE_DIR,
                    version: 1,
                };
                empty_root.compute_and_store(db, repo_id).await?
            }
        }
    } else {
        let head_root_id = get_head_root_id(db, repo_id).await?;
        crate::repo::resolve_fs_id(repos, repo_id, &head_root_id, &parent_path)
            .await
            .map_err(|e| AppError::Internal(format!("resolve parent failed: {e}")))?
    };

    let email_clone = email.to_string();
    let dir_name_clone = dir_name.to_string();
    FileOps::update_dir_tree_and_commit(
        db,
        repos,
        repo_id,
        &parent_path,
        &parent_fs_id,
        email,
        &format!("Created directory {dir_name}"),
        crate::repo::file_ops::EMPTY_ANCESTOR_CHAIN,
        |dirents| {
            if !dirents.iter().any(|d| d.name == dir_name_clone) {
                dirents.push(DirEntryData {
                    id: dir_fs_id.clone(),
                    mode: S_IFDIR,
                    modifier: email_clone.clone(),
                    mtime: now,
                    name: dir_name_clone.clone(),
                    size: 0,
                });
            }
            Ok(())
        },
    )
    .await
    .map_err(|e| AppError::Internal(e.to_string()))?;

    activity_log::log_activity(
        db, repo_id, "create", "dir", &path, user_id, None, None, None, None, None,
    )
    .await;

    Ok(())
}

// ── Helpers ─────────────────────────────────────────────────────────────

async fn read_fs_dir_data(
    repos: &Repositories,
    repo_id: &str,
    fs_id: &str,
) -> Result<FsDirData, AppError> {
    crate::repo::read_fs_dir_data(repos, repo_id, fs_id)
        .await
        .map_err(|e| AppError::internal(format!("read fs_dir_data failed: {e}")))
}

async fn rename_dir_entry(
    db: &DatabaseConnection,
    repos: &Repositories,
    repo_id: &str,
    path: &str,
    new_name: &str,
    modifier: &str,
    user_id: i32,
) -> Result<(), AppError> {
    let parent_path = parent_path_from(path);
    let old_name = path.rsplit_once('/').map(|(_, n)| n).unwrap_or("");

    let head_root_id = get_head_root_id(db, repo_id).await?;
    let parent_fs_id = crate::repo::resolve_fs_id(repos, repo_id, &head_root_id, parent_path)
        .await
        .map_err(|e| AppError::Internal(format!("resolve parent failed: {e}")))?;

    let parent_data = crate::repo::read_fs_dir_data(repos, repo_id, &parent_fs_id)
        .await
        .map_err(|e| AppError::Internal(format!("read parent failed: {e}")))?;
    let child_id = parent_data
        .dirents
        .iter()
        .find(|d| d.name == old_name)
        .map(|d| d.id.clone())
        .ok_or_else(|| AppError::NotFound("directory not found".into()))?;

    FileOps::update_dir_tree_and_commit(
        db,
        repos,
        repo_id,
        parent_path,
        &parent_fs_id,
        modifier,
        &format!("Renamed directory {old_name}"),
        crate::repo::file_ops::EMPTY_ANCESTOR_CHAIN,
        |dirents| {
            if let Some(d) = dirents.iter_mut().find(|d| d.id == child_id) {
                d.name = new_name.to_string();
            }
            Ok(())
        },
    )
    .await
    .map_err(|e| AppError::Internal(e.to_string()))?;

    let new_path = if parent_path == "/" {
        format!("/{new_name}")
    } else {
        format!("{parent_path}/{new_name}")
    };
    activity_log::log_activity(
        db,
        repo_id,
        "rename",
        "dir",
        &new_path,
        user_id,
        Some(path),
        None,
        None,
        None,
        None,
    )
    .await;

    // Update starred items with the new path (dir and all items inside)
    if let Err(e) = db
        .execute(sea_orm::Statement::from_sql_and_values(
            sea_orm::DatabaseBackend::Sqlite,
            "UPDATE starred_files SET path = $1 || substr(path, length($2) + 1) \
             WHERE repo_id = $3 AND (path = $2 OR path LIKE $2 || '/%')",
            vec![
                new_path.clone().into(),
                path.to_owned().into(),
                repo_id.to_owned().into(),
            ],
        ))
        .await
    {
        tracing::warn!("Failed to update starred paths for directory {path}: {e}");
    }

    Ok(())
}

// ── DirService ──────────────────────────────────────────────────────────

pub struct DirService {
    repos: Arc<Repositories>,
    db: Arc<DatabaseConnection>,
    indexer: Option<crate::indexer::TextIndexer>,
}

impl DirService {
    pub fn new(
        repos: Arc<Repositories>,
        db: Arc<DatabaseConnection>,
        indexer: Option<crate::indexer::TextIndexer>,
    ) -> Self {
        Self { repos, db, indexer }
    }

    pub fn db(&self) -> &DatabaseConnection {
        self.db.as_ref()
    }

    /// Single-level directory listing.
    pub async fn list_dir(
        &self,
        repo_id: &str,
        path: &str,
    ) -> Result<(String, Vec<DirEntry>), AppError> {
        list_dir_from_fs_tree(self.db(), &self.repos, repo_id, path).await
    }

    /// Recursive directory listing.
    pub async fn list_dir_recursive(
        &self,
        repo_id: &str,
        path: &str,
    ) -> Result<(String, Vec<DirEntry>), AppError> {
        list_dir_recursive_from_fs_tree(self.db(), &self.repos, repo_id, path).await
    }

    /// Create a directory.
    pub async fn create_dir(
        &self,
        repo_id: &str,
        path: &str,
        email: &str,
        user_id: i32,
    ) -> Result<(), AppError> {
        create_dir_by_path(self.db(), &self.repos, email, user_id, repo_id, path).await
    }

    /// Rename a directory entry.
    pub async fn rename_dir_entry(
        &self,
        repo_id: &str,
        path: &str,
        new_name: &str,
        modifier: &str,
        user_id: i32,
    ) -> Result<(), AppError> {
        rename_dir_entry(
            self.db(),
            &self.repos,
            repo_id,
            path,
            new_name,
            modifier,
            user_id,
        )
        .await
    }

    /// Delete a directory.
    pub async fn delete_dir(
        &self,
        repo_id: &str,
        path: &str,
        email: &str,
        user_id: i32,
    ) -> Result<(), AppError> {
        let db = self.db();
        let name = path.rsplit_once('/').map(|(_, n)| n).unwrap_or("");
        let parent_path = parent_path_from(path);

        let deleted_size = crate::repo::get_entry_total_size(db, &self.repos, repo_id, path)
            .await
            .ok()
            .unwrap_or(0);

        let head_root_id = get_head_root_id(db, repo_id).await?;
        let parent_fs_id =
            crate::repo::resolve_fs_id(&self.repos, repo_id, &head_root_id, parent_path)
                .await
                .map_err(|e| AppError::Internal(format!("resolve parent failed: {e}")))?;

        // Record deleted entry to trash before tree update
        record_delete_trash(db, &self.repos, repo_id, path, name, email, &parent_fs_id).await;

        FileOps::update_dir_tree_and_commit(
            db,
            &self.repos,
            repo_id,
            parent_path,
            &parent_fs_id,
            email,
            &format!("Deleted directory {name}"),
            crate::repo::file_ops::EMPTY_ANCESTOR_CHAIN,
            |dirents| {
                dirents.retain(|d| d.name != name);
                Ok(())
            },
        )
        .await
        .map_err(|e| AppError::Internal(e.to_string()))?;

        crate::repo::adjust_repo_size(db, &self.repos, repo_id, -deleted_size).await?;

        activity_log::log_activity(
            db, repo_id, "delete", "dir", path, user_id, None, None, None, None, None,
        )
        .await;

        Ok(())
    }

    /// Move a directory.
    pub async fn move_dir(
        &self,
        repo_id: &str,
        path: &str,
        new_parent_dir: &str,
        email: &str,
        user_id: i32,
    ) -> Result<(), AppError> {
        let db = self.db();

        let head_root_id = get_head_root_id(db, repo_id).await?;
        let parent_path = parent_path_from(path);
        let dir_name = path.rsplit_once('/').map(|(_, n)| n).unwrap_or("");

        let old_parent_fs_id =
            crate::repo::resolve_fs_id(&self.repos, repo_id, &head_root_id, parent_path)
                .await
                .map_err(|e| AppError::Internal(format!("resolve old parent failed: {e}")))?;

        let old_parent_data =
            crate::repo::read_fs_dir_data(&self.repos, repo_id, &old_parent_fs_id)
                .await
                .map_err(|e| AppError::Internal(format!("read old parent failed: {e}")))?;
        let entry = old_parent_data
            .dirents
            .iter()
            .find(|d| d.name == dir_name)
            .ok_or_else(|| AppError::NotFound("directory not found".into()))?;

        let dir_fs_id = entry.id.clone();
        let dir_mode = entry.mode;
        let dir_size = entry.size;

        // new_parent_dir should already be validated by handler, but we use safe_normalize_path
        // for defensive programming. If it fails, it's an internal error (handler bug).
        let new_parent_path = crate::sanitize::safe_normalize_path(new_parent_dir)
            .map_err(|e| AppError::Internal(format!("path normalization failed: {e}")))?;
        let _new_parent_fs_id =
            crate::repo::resolve_fs_id(&self.repos, repo_id, &head_root_id, &new_parent_path)
                .await
                .map_err(|e| AppError::Internal(format!("resolve dest parent failed: {e}")))?;

        let intermediate_root = FileOps::update_dir_tree_no_commit(
            db,
            &self.repos,
            repo_id,
            parent_path,
            &old_parent_fs_id,
            crate::repo::file_ops::EMPTY_ANCESTOR_CHAIN,
            |dirents| {
                dirents.retain(|d| d.name != dir_name);
                Ok(())
            },
        )
        .await
        .map_err(|e| AppError::Internal(e.to_string()))?;

        FileOps::create_commit(
            db,
            &self.repos,
            repo_id,
            &intermediate_root,
            email,
            &format!("Moved directory {dir_name}"),
        )
        .await
        .map_err(|e| AppError::Internal(e.to_string()))?;

        let new_head_root = get_head_root_id(db, repo_id).await?;
        let new_dst_fs_id =
            crate::repo::resolve_fs_id(&self.repos, repo_id, &new_head_root, &new_parent_path)
                .await
                .map_err(|e| {
                    AppError::Internal(format!("resolve dest dir after removal failed: {e}"))
                })?;

        let now = chrono::Utc::now().timestamp();
        let email_clone = email.to_string();
        let dir_name_clone = dir_name.to_string();
        FileOps::update_dir_tree_and_commit(
            db,
            &self.repos,
            repo_id,
            &new_parent_path,
            &new_dst_fs_id,
            email,
            &format!("Moved directory {dir_name}"),
            crate::repo::file_ops::EMPTY_ANCESTOR_CHAIN,
            |dirents| {
                if !dirents.iter().any(|d| d.name == dir_name_clone) {
                    dirents.push(DirEntryData {
                        id: dir_fs_id.clone(),
                        mode: dir_mode,
                        modifier: email_clone.clone(),
                        mtime: now,
                        name: dir_name_clone.clone(),
                        size: dir_size,
                    });
                }
                Ok(())
            },
        )
        .await
        .map_err(|e| AppError::Internal(e.to_string()))?;

        let new_path = if new_parent_path == "/" {
            format!("/{dir_name}")
        } else {
            format!("{new_parent_path}/{dir_name}")
        };
        activity_log::log_activity(
            db,
            repo_id,
            "move",
            "dir",
            &new_path,
            user_id,
            Some(path),
            None,
            None,
            None,
            None,
        )
        .await;

        Ok(())
    }

    /// Get shared items for a directory.
    pub async fn get_dir_shared_items(
        &self,
        repo_id: &str,
        path: &str,
    ) -> Result<Vec<serde_json::Value>, AppError> {
        let links = self
            .repos
            .share_link
            .find_by_repo_and_path(repo_id, path)
            .await?;

        Ok(links
            .into_iter()
            .map(|l| {
                serde_json::json!({
                    "share_type": "download",
                    "token": l.token,
                    "path": l.path,
                    "repo_id": l.repo_id,
                    "creator_email": "",
                    "created_at": l.created_at,
                })
            })
            .collect())
    }

    /// Create a sub-repository from a directory.
    pub async fn create_sub_repo(
        &self,
        repo_id: &str,
        path: &str,
        email: &str,
        user_id: i32,
    ) -> Result<serde_json::Value, AppError> {
        let db = self.db();
        let head_root_id = get_head_root_id(db, repo_id).await?;
        let source_dir_fs_id =
            crate::repo::resolve_fs_id(&self.repos, repo_id, &head_root_id, path)
                .await
                .map_err(|_| AppError::NotFound("directory not found".into()))?;

        let new_repo_id = uuid::Uuid::new_v4().to_string();
        let now = chrono::Utc::now().timestamp();
        let dir_name = path
            .trim_matches('/')
            .rsplit('/')
            .next()
            .unwrap_or("subrepo");

        let model = repo::ActiveModel {
            id: sea_orm::Set(new_repo_id.clone()),
            name: sea_orm::Set(dir_name.to_string()),
            description: sea_orm::Set(String::new()),
            owner_id: sea_orm::Set(user_id),
            encrypted: sea_orm::Set(0i8),
            enc_version: sea_orm::Set(0i8),
            magic: sea_orm::Set(None),
            random_key: sea_orm::Set(None),
            salt: sea_orm::Set(String::new()),
            head_commit_id: sea_orm::NotSet,
            permission: sea_orm::Set("rw".to_string()),
            repo_version: sea_orm::Set(1),
            size: sea_orm::Set(0),
            created_at: sea_orm::Set(now),
            updated_at: sea_orm::Set(now),
        };
        self.repos.repo.create(model).await?;

        self.repos
            .member
            .create(repo_member::ActiveModel {
                id: sea_orm::NotSet,
                repo_id: sea_orm::Set(new_repo_id.clone()),
                user_id: sea_orm::Set(user_id),
                permission: sea_orm::Set("rw".to_string()),
                created_at: sea_orm::Set(now),
            })
            .await?;

        copy_fs_tree(&self.repos, repo_id, &new_repo_id, &source_dir_fs_id).await?;

        FileOps::create_commit(
            db,
            &self.repos,
            &new_repo_id,
            &source_dir_fs_id,
            email,
            "Created sub-repo",
        )
        .await
        .map_err(|e| AppError::Internal(format!("create commit failed: {e}")))?;

        Ok(serde_json::json!({
            "id": new_repo_id,
            "name": dir_name,
            "desc": "",
            "size": 0,
            "encrypted": 0,
            "enc_version": 0,
            "owner": email,
            "permission": "rw",
            "mtime": now,
        }))
    }

    /// Delete a directory entry (v21 API, used for both files and directories).
    /// Returns (deleted_size, obj_type_str).
    pub async fn delete_dirent(
        &self,
        repo_id: &str,
        obj: &str,
        path: &str,
        email: &str,
        user_id: i32,
    ) -> Result<(), AppError> {
        let db = self.db();
        let name = path.rsplit_once('/').map(|(_, n)| n).unwrap_or("");
        let parent_path = parent_path_from(path);

        let repo_model = self
            .repos
            .repo
            .find_by_id(repo_id)
            .await?
            .ok_or_else(|| AppError::NotFound("repo not found".into()))?;
        let head_commit_id = repo_model
            .head_commit_id
            .ok_or_else(|| AppError::NotFound("no commits".into()))?;
        let head_commit = self
            .repos
            .commit
            .find_by_id(&head_commit_id)
            .await?
            .ok_or_else(|| AppError::NotFound("head commit not found".into()))?;

        let parent_fs_id =
            crate::repo::resolve_fs_id(&self.repos, repo_id, &head_commit.root_id, parent_path)
                .await
                .map_err(|e| AppError::Internal(format!("resolve parent failed: {e}")))?;

        let deleted_size: i64 = crate::repo::get_entry_total_size(db, &self.repos, repo_id, path)
            .await
            .unwrap_or_default();

        // Trash: record deleted entry before tree update
        if let Ok(parent_dir_data) =
            crate::repo::read_fs_dir_data(&self.repos, repo_id, &parent_fs_id).await
            && let Some(entry) = parent_dir_data.dirents.iter().find(|d| d.name == name)
        {
            let obj_type = if entry.mode & S_IFDIR != 0 {
                "dir"
            } else {
                "file"
            };
            if let Err(e) = crate::repo::trash::TrashService::add_to_trash(
                db,
                repo_id,
                path,
                obj_type,
                &entry.id,
                &entry.name,
                entry.size,
                &head_commit_id,
                email,
            )
            .await
            {
                tracing::warn!("Failed to record trash for {path}: {e}");
            }
        }

        FileOps::update_dir_tree_and_commit(
            db,
            &self.repos,
            repo_id,
            parent_path,
            &parent_fs_id,
            email,
            &format!("Deleted {name}"),
            crate::repo::file_ops::EMPTY_ANCESTOR_CHAIN,
            |dirents| {
                dirents.retain(|d| d.name != name);
                Ok(())
            },
        )
        .await
        .map_err(|e| AppError::Internal(e.to_string()))?;

        // Remove from full-text search index
        if let Some(indexer) = &self.indexer
            && let Err(e) = indexer.delete_file(repo_id, path)
        {
            tracing::warn!("Failed to delete index for {path}: {e}");
        }

        crate::repo::adjust_repo_size(db, &self.repos, repo_id, -deleted_size).await?;

        activity_log::log_activity(
            db, repo_id, "delete", obj, path, user_id, None, None, None, None, None,
        )
        .await;

        Ok(())
    }

    /// Build the v21 directory listing response data (non-recursive), including starred info.
    pub async fn build_list_dir_v21_json(
        &self,
        repo_id: &str,
        path: &str,
        user_id: i32,
        with_thumbnail: bool,
        entries: Vec<DirEntry>,
        dir_id: String,
    ) -> Result<serde_json::Value, AppError> {
        let _db = self.db();

        let user_perm = self
            .repos
            .member
            .find_by_repo_and_user(repo_id, user_id)
            .await?
            .map(|m| m.permission)
            .unwrap_or_else(|| "rw".to_string());

        let mut dir_list: Vec<_> = Vec::new();
        let mut file_list: Vec<_> = Vec::new();
        for e in entries {
            if e.entry_type == "dir" {
                dir_list.push(e);
            } else {
                file_list.push(e);
            }
        }
        dir_list.sort_by_key(|a| a.name.to_lowercase());
        file_list.sort_by_key(|a| a.name.to_lowercase());

        let starred_set: HashSet<String> = self
            .repos
            .starred
            .find_by_user_and_repo(user_id, repo_id)
            .await?
            .into_iter()
            .map(|s| s.path.trim_end_matches('/').to_string())
            .collect();

        let parent_dir = if path == "/" {
            path.to_string()
        } else {
            format!("{}/", path.trim_end_matches('/'))
        };

        let mut dirent_list = Vec::with_capacity(dir_list.len() + file_list.len());

        for e in &dir_list {
            let entry_path = format!("{}{}", parent_dir, e.name);
            dirent_list.push(serde_json::json!({
                "type": "dir",
                "id": e.id,
                "name": e.name,
                "mtime": e.mtime,
                "permission": e.permission,
                "parent_dir": parent_dir,
                "starred": starred_set.contains(&entry_path),
            }));
        }

        let mut nickname_cache: HashMap<String, String> = HashMap::new();
        let modifier_emails: Vec<String> = file_list.iter().map(|e| e.modifier.clone()).collect();
        for email in &modifier_emails {
            if !email.is_empty() {
                let name = self
                    .repos
                    .user
                    .find_by_email(email)
                    .await?
                    .map(|u| u.nickname())
                    .unwrap_or_else(|| email.split('@').next().unwrap_or("").to_string());
                nickname_cache.insert(email.clone(), name);
            }
        }

        for e in &file_list {
            let modifier_email = e.modifier.as_str();
            let modifier_name = nickname_cache
                .get(modifier_email)
                .map(|s| s.as_str())
                .unwrap_or_else(|| modifier_email.split('@').next().unwrap_or(""));
            let modifier_contact_email = modifier_email;
            let entry_path = format!("{}{}", parent_dir, e.name);
            let mut entry = serde_json::json!({
                "type": "file",
                "id": e.id,
                "name": e.name,
                "size": e.size,
                "mtime": e.mtime,
                "permission": e.permission,
                "parent_dir": parent_dir,
                "starred": starred_set.contains(&entry_path),
                "modifier_email": modifier_email,
                "modifier_name": modifier_name,
                "modifier_contact_email": modifier_contact_email,
            });
            if with_thumbnail {
                entry["encoded_thumbnail_src"] = serde_json::Value::String(String::new());
            }
            dirent_list.push(entry);
        }

        Ok(serde_json::json!({
            "user_perm": user_perm,
            "dir_id": dir_id,
            "dirent_list": dirent_list,
        }))
    }

    /// Get directory detail (v21 API).
    pub async fn dir_detail(
        &self,
        repo_id: &str,
        path: &str,
        user_id: i32,
    ) -> Result<serde_json::Value, AppError> {
        let _db = self.db();
        let repo_record = self
            .repos
            .repo
            .find_by_id(repo_id)
            .await?
            .ok_or_else(|| AppError::NotFound("Library not found".into()))?;
        let head_commit_id = repo_record
            .head_commit_id
            .ok_or_else(|| AppError::NotFound("no commits".into()))?;
        let head_commit = self
            .repos
            .commit
            .find_by_id(&head_commit_id)
            .await?
            .ok_or_else(|| AppError::NotFound("head commit not found".into()))?;

        crate::repo::resolve_fs_id(&self.repos, repo_id, &head_commit.root_id, path)
            .await
            .map_err(|_| AppError::NotFound("Folder not found.".into()))?;

        let dir_name = path
            .trim_end_matches('/')
            .rsplit_once('/')
            .map(|(_, n)| n)
            .unwrap_or("");

        let parent_path = match path.trim_end_matches('/').rsplit_once('/') {
            Some(("", _)) => "/",
            Some((parent, _)) => parent,
            None => "/",
        };

        let mtime = if parent_path == "/" {
            let root_data =
                crate::repo::read_fs_dir_data(&self.repos, repo_id, &head_commit.root_id)
                    .await
                    .unwrap_or_else(|_| FsDirData {
                        dirents: vec![],
                        obj_type: SEAF_METADATA_TYPE_DIR,
                        version: 1,
                    });
            root_data
                .dirents
                .iter()
                .find(|d| d.name == dir_name)
                .map(|d| d.mtime)
                .unwrap_or(0)
        } else {
            let parent_fs_id = match crate::repo::resolve_fs_id(
                &self.repos,
                repo_id,
                &head_commit.root_id,
                parent_path,
            )
            .await
            {
                Ok(id) => id,
                Err(_) => return Err(AppError::NotFound("Folder not found.".into())),
            };
            let parent_data = crate::repo::read_fs_dir_data(&self.repos, repo_id, &parent_fs_id)
                .await
                .map_err(|e| AppError::Internal(format!("read parent failed: {e}")))?;
            parent_data
                .dirents
                .iter()
                .find(|d| d.name == dir_name)
                .map(|d| d.mtime)
                .unwrap_or(0)
        };

        let permission = self
            .repos
            .member
            .find_by_repo_and_user(repo_id, user_id)
            .await?
            .map(|m| m.permission)
            .unwrap_or_else(|| "rw".to_string());

        Ok(serde_json::json!({
            "repo_id": repo_id,
            "path": path,
            "name": dir_name,
            "mtime": mtime,
            "permission": permission,
        }))
    }
}

// ── Helpers (private) ───────────────────────────────────────────────

/// Copy all reachable fs_objects from one repo to another.
async fn copy_fs_tree(
    repos: &Repositories,
    src_repo_id: &str,
    dst_repo_id: &str,
    root_fs_id: &str,
) -> Result<(), AppError> {
    let mut stack = vec![root_fs_id.to_string()];
    while let Some(fs_id) = stack.pop() {
        if fs_id == "0000000000000000000000000000000000000000" {
            continue;
        }
        let obj = repos
            .fs_object
            .find_by_repo_and_fs_id(src_repo_id, &fs_id)
            .await?
            .ok_or_else(|| AppError::NotFound(format!("fs_object not found: {fs_id}")))?;

        let exists = repos
            .fs_object
            .exists_by_repo_and_fs_id(dst_repo_id, &fs_id)
            .await?;
        if !exists {
            repos
                .fs_object
                .insert_many(vec![crate::entity::fs_object::ActiveModel {
                    id: sea_orm::NotSet,
                    repo_id: sea_orm::Set(dst_repo_id.to_string()),
                    fs_id: sea_orm::Set(fs_id.clone()),
                    obj_type: sea_orm::Set(obj.obj_type),
                    data: sea_orm::Set(obj.data.clone()),
                }])
                .await?;
        }

        if obj.obj_type == SEAF_METADATA_TYPE_DIR as i8 {
            let dir_data: FsDirData = serde_json::from_str(&obj.data)
                .map_err(|e| AppError::Internal(format!("deserialize failed: {e}")))?;
            for entry in &dir_data.dirents {
                stack.push(entry.id.clone());
            }
        }
    }
    Ok(())
}

/// Record a deleted entry to the trash table.
async fn record_delete_trash(
    db: &sea_orm::DatabaseConnection,
    repos: &Repositories,
    repo_id: &str,
    path: &str,
    name: &str,
    email: &str,
    parent_fs_id: &str,
) {
    let head_commit_id = match get_head_commit_id(db, repo_id).await {
        Ok(id) => id,
        Err(_) => return,
    };
    let parent_dir_data = match crate::repo::read_fs_dir_data(repos, repo_id, parent_fs_id).await {
        Ok(d) => d,
        Err(_) => return,
    };
    let entry = match parent_dir_data.dirents.iter().find(|d| d.name == name) {
        Some(e) => e,
        None => return,
    };
    let obj_type = if entry.mode & S_IFDIR != 0 {
        "dir"
    } else {
        "file"
    };
    if let Err(e) = crate::repo::trash::TrashService::add_to_trash(
        db,
        repo_id,
        path,
        obj_type,
        &entry.id,
        &entry.name,
        entry.size,
        &head_commit_id,
        email,
    )
    .await
    {
        tracing::warn!("Failed to record trash for {path}: {e}");
    }
}
