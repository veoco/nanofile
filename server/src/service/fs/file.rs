use std::sync::Arc;

use sea_orm::DatabaseConnection;

use crate::fs::core::file_ops::FileOps;
use crate::fs::core::trash;
use crate::notification::events::FileLockEvent;
use crate::repository::Repositories;
use base::common::{DirEntryData, FsDirData, FsFileData, SEAF_METADATA_TYPE_DIR};
use base::error::AppError;
use infra::activity_log;
use infra::common::util::{
    basename, get_head_root_id, get_head_root_id_opt, join_path, parent_path_from,
    timestamp_rfc3339,
};
use infra::serialization::{S_IFDIR, S_IFREG};

/// Parsed upload data, extracted from multipart at the handler layer.
pub struct UploadedFile {
    pub file_name: String,
    pub file_data: Vec<u8>,
    pub parent_dir: String,
    pub replace: bool,
}

// ── Rename entry (pub(crate), shared by file and dir services) ───────────

/// Rename a file or directory entry in place.
///
/// `is_dir` controls the commit description, activity type and error wording;
/// filenames are validated for files only (preserving legacy behavior).
pub(crate) async fn rename_entry(
    db: &DatabaseConnection,
    repos: &Repositories,
    repo_id: &str,
    path: &str,
    new_name: &str,
    modifier: &str,
    user_id: i32,
    is_dir: bool,
) -> Result<(), AppError> {
    if !is_dir {
        base::sanitize::validate_filename(new_name)
            .map_err(|e| AppError::BadRequest(format!("invalid filename: {e}")))?;
    }

    let parent_path = parent_path_from(path);
    let old_name = basename(path);

    let head_root_id = get_head_root_id(db, repo_id).await?;
    let parent_fs_id = crate::fs::core::resolve_fs_id(repos, repo_id, &head_root_id, parent_path)
        .await
        .map_err(|e| AppError::Internal(format!("resolve parent failed: {e}")))?;

    let parent_data = crate::fs::core::read_fs_dir_data(repos, repo_id, &parent_fs_id)
        .await
        .map_err(|e| AppError::Internal(format!("read parent failed: {e}")))?;
    let child_id = parent_data
        .dirents
        .iter()
        .find(|d| d.name == old_name)
        .map(|d| d.id.clone())
        .ok_or_else(|| {
            AppError::NotFound(if is_dir {
                "directory not found".into()
            } else {
                "file not found".into()
            })
        })?;

    FileOps::update_dir_tree_and_commit(
        db,
        repos,
        repo_id,
        parent_path,
        &parent_fs_id,
        modifier,
        &if is_dir {
            format!("Renamed directory {old_name}")
        } else {
            format!("Renamed {old_name}")
        },
        crate::fs::core::file_ops::EMPTY_ANCESTOR_CHAIN,
        |dirents| {
            if let Some(d) = dirents.iter_mut().find(|d| d.id == child_id) {
                d.name = new_name.to_string();
            }
            Ok(())
        },
    )
    .await
    .map_err(|e| AppError::Internal(e.to_string()))?;

    let new_path = join_path(parent_path, new_name);
    activity_log::log_activity(
        db,
        repo_id,
        "rename",
        if is_dir { "dir" } else { "file" },
        &new_path,
        user_id,
        Some(path),
        None,
        None,
        None,
        None,
    )
    .await;

    // Update starred items with the new path (dirs move all items inside).
    if let Err(e) = repos
        .starred
        .update_paths_for_rename(path, &new_path, repo_id)
        .await
    {
        tracing::warn!("Failed to update starred path for {path}: {e}");
    }

    Ok(())
}

// ── Move entry (shared by file and dir services) ─────────────────────────

/// Move a file or directory to a new parent directory using the two-phase
/// commit sequence (remove from old parent, commit, re-add to new parent).
///
/// `is_dir` controls the commit description, activity type and error wording.
/// The caller is responsible for index updates (files only).
pub(crate) async fn move_entry(
    db: &DatabaseConnection,
    repos: &Repositories,
    repo_id: &str,
    path: &str,
    new_parent_dir: &str,
    email: &str,
    user_id: i32,
    is_dir: bool,
) -> Result<String, AppError> {
    let head_root_id = get_head_root_id(db, repo_id).await?;
    let entry_name = basename(path);
    let parent_path = parent_path_from(path);

    let old_parent_fs_id =
        crate::fs::core::resolve_fs_id(repos, repo_id, &head_root_id, parent_path)
            .await
            .map_err(|e| AppError::Internal(format!("resolve old parent failed: {e}")))?;

    let old_parent_data = crate::fs::core::read_fs_dir_data(repos, repo_id, &old_parent_fs_id)
        .await
        .map_err(|e| AppError::Internal(format!("read old parent failed: {e}")))?;
    let entry = old_parent_data
        .dirents
        .iter()
        .find(|d| d.name == entry_name)
        .ok_or_else(|| {
            AppError::NotFound(if is_dir {
                "directory not found".into()
            } else {
                "file not found".into()
            })
        })?;

    let entry_fs_id = entry.id.clone();
    let entry_mode = entry.mode;
    let entry_size = entry.size;

    // new_parent_dir should already be validated by handler, but we use
    // safe_normalize_path for defensive programming. If it fails, it's an
    // internal error (handler bug).
    let new_parent_path = base::sanitize::safe_normalize_path(new_parent_dir)
        .map_err(|e| AppError::Internal(format!("path normalization failed: {e}")))?;
    let _ = crate::fs::core::resolve_fs_id(repos, repo_id, &head_root_id, &new_parent_path)
        .await
        .map_err(|e| AppError::Internal(format!("resolve dest parent failed: {e}")))?;

    let intermediate_root = FileOps::update_dir_tree_no_commit(
        db,
        repos,
        repo_id,
        parent_path,
        &old_parent_fs_id,
        crate::fs::core::file_ops::EMPTY_ANCESTOR_CHAIN,
        |dirents| {
            dirents.retain(|d| d.name != entry_name);
            Ok(())
        },
    )
    .await
    .map_err(|e| AppError::Internal(e.to_string()))?;

    FileOps::create_commit(
        repos,
        repo_id,
        &intermediate_root,
        email,
        &if is_dir {
            format!("Moved directory {entry_name}")
        } else {
            format!("Moved {entry_name}")
        },
    )
    .await
    .map_err(|e| AppError::Internal(e.to_string()))?;

    let new_head_root = get_head_root_id(db, repo_id).await?;
    let new_dst_fs_id =
        crate::fs::core::resolve_fs_id(repos, repo_id, &new_head_root, &new_parent_path)
            .await
            .map_err(|e| {
                AppError::Internal(format!("resolve dest dir after removal failed: {e}"))
            })?;

    let now = chrono::Utc::now().timestamp();
    let email_clone = email.to_string();
    let entry_name_clone = entry_name.to_string();
    FileOps::update_dir_tree_and_commit(
        db,
        repos,
        repo_id,
        &new_parent_path,
        &new_dst_fs_id,
        email,
        &if is_dir {
            format!("Moved directory {entry_name}")
        } else {
            format!("Moved {entry_name}")
        },
        crate::fs::core::file_ops::EMPTY_ANCESTOR_CHAIN,
        |dirents| {
            if !dirents.iter().any(|d| d.name == entry_name_clone) {
                dirents.push(DirEntryData {
                    id: entry_fs_id.clone(),
                    mode: entry_mode,
                    modifier: email_clone.clone(),
                    mtime: now,
                    name: entry_name_clone.clone(),
                    size: entry_size,
                });
            }
            Ok(())
        },
    )
    .await
    .map_err(|e| AppError::Internal(e.to_string()))?;

    let new_path = join_path(&new_parent_path, entry_name);
    activity_log::log_activity(
        db,
        repo_id,
        "move",
        if is_dir { "dir" } else { "file" },
        &new_path,
        user_id,
        Some(path),
        None,
        None,
        None,
        None,
    )
    .await;

    Ok(new_path)
}

// ── FileService ─────────────────────────────────────────────────────────

pub struct FileService {
    repos: Arc<Repositories>,
    db: Arc<DatabaseConnection>,
    block_store: infra::storage::DynBlockStorage,
    indexer: Option<crate::indexer::TextIndexer>,
    token_manager: Arc<crate::AccessTokenManager>,
    config: Arc<infra::config::Config>,
    notification_manager: Option<crate::notification::manager::NotificationManager>,
}

impl FileService {
    pub fn new(
        repos: Arc<Repositories>,
        db: Arc<DatabaseConnection>,
        block_store: infra::storage::DynBlockStorage,
        indexer: Option<crate::indexer::TextIndexer>,
        token_manager: Arc<crate::AccessTokenManager>,
        config: Arc<infra::config::Config>,
        notification_manager: Option<crate::notification::manager::NotificationManager>,
    ) -> Self {
        Self {
            repos,
            db,
            block_store,
            indexer,
            token_manager,
            config,
            notification_manager,
        }
    }

    fn db(&self) -> &DatabaseConnection {
        self.db.as_ref()
    }

    /// Generate a download URL and return the file_fs_id.
    pub async fn get_download_info(
        &self,
        repo_id: &str,
        path: &str,
        user_id: i32,
        email: &str,
        host_header: Option<&str>,
    ) -> Result<(String, String), AppError> {
        let db = self.db();
        let head_root_id = get_head_root_id(db, repo_id).await?;
        let file_fs_id = crate::fs::core::resolve_fs_id(&self.repos, repo_id, &head_root_id, path)
            .await
            .map_err(|_| AppError::NotFound("file not found".into()))?;

        let filename = path.rsplit_once('/').map(|(_, n)| n).unwrap_or("download");
        let download_token = self.token_manager.generate_download(
            repo_id,
            user_id,
            email,
            path,
            &file_fs_id,
            filename,
        );

        let url = self.build_download_url(&download_token, host_header);
        Ok((file_fs_id, url))
    }

    /// Build a download API URL.
    fn build_download_url(&self, token: &str, host_header: Option<&str>) -> String {
        if let Some(h) = host_header {
            let scheme = self.config.server.site_url_scheme();
            if let Some((h, p)) = h.split_once(':') {
                format!("{scheme}://{h}:{p}/download-api/{token}")
            } else {
                format!(
                    "{scheme}://{h}:{}/download-api/{token}",
                    self.config.server.port
                )
            }
        } else {
            let base = self.config.server.site_url.trim_end_matches('/');
            format!("{base}/download-api/{token}")
        }
    }

    /// Build a block download URL.
    fn build_block_download_url(
        &self,
        token: &str,
        file_id: &str,
        block_id: &str,
        host_header: Option<&str>,
    ) -> String {
        if let Some(h) = host_header {
            let scheme = self.config.server.site_url_scheme();
            if let Some((h, p)) = h.split_once(':') {
                format!("{scheme}://{h}:{p}/blks/{token}/{file_id}/{block_id}")
            } else {
                format!(
                    "{scheme}://{h}:{}/blks/{token}/{file_id}/{block_id}",
                    self.config.server.port
                )
            }
        } else {
            let base = self.config.server.site_url.trim_end_matches('/');
            format!("{base}/blks/{token}/{file_id}/{block_id}")
        }
    }

    /// Get a block download link.
    pub async fn get_block_download_link(
        &self,
        repo_id: &str,
        file_id: &str,
        block_id: &str,
        parent_dir: &str,
        user_id: i32,
        email: &str,
        host_header: Option<&str>,
    ) -> Result<String, AppError> {
        let token =
            self.token_manager
                .generate(repo_id, user_id, email, "downloadblks", parent_dir);
        Ok(self.build_block_download_url(&token, file_id, block_id, host_header))
    }

    /// Upload a file from pre-parsed upload data.
    pub async fn upload_file(
        &self,
        repo_id: &str,
        upload: UploadedFile,
        email: &str,
        user_id: i32,
    ) -> Result<(), AppError> {
        let file_data = upload.file_data;
        let file_name = upload.file_name;
        let parent_dir = upload.parent_dir;
        let replace = upload.replace;

        let _fs_id = self
            .upload_file_committed(
                repo_id,
                &parent_dir,
                &file_name,
                &file_data,
                email,
                Some(user_id),
                None,
                false,
                Some(replace),
            )
            .await?;

        // Full-text index the uploaded file (web uploads index lazily on read).
        if let Some(indexer) = &self.indexer {
            let full_path = if parent_dir.ends_with('/') {
                format!("{parent_dir}{file_name}")
            } else if parent_dir == "/" {
                format!("/{file_name}")
            } else {
                format!("{parent_dir}/{file_name}")
            };
            if crate::indexer::is_indexable_text(&file_name, &file_data) {
                let content = String::from_utf8_lossy(&file_data);
                if let Err(e) = indexer.index_file(repo_id, &full_path, &file_name, &content) {
                    tracing::warn!("Failed to index file {file_name}: {e}");
                }
            } else if replace && let Err(e) = indexer.delete_file(repo_id, &full_path) {
                tracing::warn!("Failed to delete index for {file_name}: {e}");
            }
        }

        Ok(())
    }

    /// Write a file into a repo and commit it, returning the new `fs_id`.
    ///
    /// This is the canonical write path shared by `upload_file` and the web
    /// upload handlers. It handles old-size detection, storage-quota checks,
    /// optional directory creation, encryption, repo-size adjustment and
    /// activity logging in one place.
    ///
    /// `replace`:
    /// - `Some(true)` — overwrite the existing entry (logged as "edit")
    /// - `Some(false)` — never overwrite
    /// - `None` — auto-detect from whether the file already exists
    pub async fn upload_file_committed(
        &self,
        repo_id: &str,
        target_dir: &str,
        filename: &str,
        data: &[u8],
        modifier: &str,
        user_id: Option<i32>,
        enc_key: Option<(&[u8], &[u8])>,
        ensure_dir: bool,
        replace: Option<bool>,
    ) -> Result<String, AppError> {
        base::sanitize::validate_filename(filename)
            .map_err(|e| AppError::BadRequest(format!("invalid filename: {e}")))?;

        let fp = base::sanitize::safe_join_path(target_dir, filename)
            .map_err(|e| AppError::BadRequest(format!("invalid path: {e}")))?;

        // Detect the existing entry to decide old-size / replace / op_type.
        let size_result = crate::fs::core::get_entry_total_size(&self.repos, repo_id, &fp).await;
        let file_exists = size_result.is_ok();
        let old_size = size_result.ok().unwrap_or(0);
        let replace_eff = replace.unwrap_or(file_exists);
        let old_size_eff = if replace_eff { old_size } else { 0 };

        // Check storage quota before accepting the upload.
        if let Some(uid) = user_id {
            crate::service::fs::quota::check_upload_quota(
                &self.repos,
                uid,
                data.len() as i64,
                self.config.storage.max_storage_bytes,
            )
            .await?;
        }

        // Ensure the target directory exists (folder uploads with missing subdirs).
        if ensure_dir && let Some(uid) = user_id {
            self.ensure_dir_recursive(repo_id, target_dir, modifier, uid)
                .await?;
        }

        let fs_id = FileOps::create_file(
            self.db(),
            &self.repos,
            repo_id,
            target_dir,
            filename,
            data,
            modifier,
            replace_eff,
            &self.block_store,
            enc_key,
        )
        .await
        .map_err(|e| AppError::Internal(format!("upload failed: {e}")))?;

        // Adjust repo size (delta = new_size - old_size).
        crate::fs::core::adjust_repo_size(&self.repos, repo_id, data.len() as i64 - old_size_eff)
            .await?;

        // Log activity.
        if let Some(uid) = user_id {
            let op_type = if replace_eff { "edit" } else { "create" };
            activity_log::log_activity(
                self.db(),
                repo_id,
                op_type,
                "file",
                &fp,
                uid,
                None,
                Some(data.len() as i64),
                Some(&fs_id),
                None,
                None,
            )
            .await;
        }

        Ok(fs_id)
    }

    /// Recursively create any missing directory components below `path`.
    async fn ensure_dir_recursive(
        &self,
        repo_id: &str,
        path: &str,
        email: &str,
        user_id: i32,
    ) -> Result<(), AppError> {
        if path == "/" {
            return Ok(());
        }

        // Quick check: if the head commit root can't be resolved, the repo is empty.
        if get_head_root_id(self.db(), repo_id).await.is_err() {
            return Ok(());
        }

        let parts: Vec<&str> = path
            .trim_start_matches('/')
            .split('/')
            .filter(|p| !p.is_empty())
            .collect();

        let mut current = String::from("/");
        for part in parts {
            let next = if current == "/" {
                format!("/{part}")
            } else {
                format!("{current}/{part}")
            };

            // Check if this component already exists.
            let root_id = get_head_root_id(self.db(), repo_id).await?;
            if crate::fs::core::resolve_fs_id(&self.repos, repo_id, &root_id, &next)
                .await
                .is_err()
            {
                crate::service::fs::dir::create_dir_by_path(
                    self.db(),
                    &self.repos,
                    email,
                    user_id,
                    repo_id,
                    &next,
                )
                .await?;
            }
            current = next;
        }

        Ok(())
    }

    /// Adjust repo size and log activity — the shared tail of the block-commit
    /// upload path (`upload_blks_api`). The caller resolves the entry state
    /// (`file_exists` / `old_size`) from the parent dirents it already holds.
    ///
    /// `op_type` is derived from whether the file actually exists, not from a
    /// client-supplied `replace` flag.
    pub async fn finalize_upload(
        &self,
        repo_id: &str,
        fp: &str,
        fs_id: &str,
        size: i64,
        user_id: Option<i32>,
        file_exists: bool,
        old_size: i64,
    ) -> Result<(), AppError> {
        crate::fs::core::adjust_repo_size(&self.repos, repo_id, size - old_size).await?;

        if let Some(uid) = user_id {
            let op_type = if file_exists { "edit" } else { "create" };
            activity_log::log_activity(
                self.db(),
                repo_id,
                op_type,
                "file",
                fp,
                uid,
                None,
                Some(size),
                Some(fs_id),
                None,
                None,
            )
            .await;
        }

        Ok(())
    }

    /// Delete a file (v2 API).
    pub async fn delete_file(
        &self,
        repo_id: &str,
        path: &str,
        email: &str,
        user_id: i32,
    ) -> Result<(), AppError> {
        let db = self.db();
        let name = basename(path);
        let parent_path = parent_path_from(path);

        let head_root_id = get_head_root_id(db, repo_id).await?;
        let parent_fs_id =
            crate::fs::core::resolve_fs_id(&self.repos, repo_id, &head_root_id, parent_path)
                .await
                .map_err(|e| AppError::Internal(format!("resolve parent failed: {e}")))?;

        // Resolve the entry size from the parent dirents (files O(1);
        // directories walk their subtree once) instead of re-resolving the path.
        let parent_data = crate::fs::core::read_fs_dir_data(&self.repos, repo_id, &parent_fs_id)
            .await
            .map_err(|e| AppError::Internal(format!("read parent dir failed: {e}")))?;
        let deleted_size = match parent_data.dirents.iter().find(|d| d.name == name) {
            Some(entry) if entry.mode & infra::serialization::S_IFDIR != 0 => {
                crate::fs::core::compute_tree_size(&self.repos, repo_id, &entry.id).await?
            }
            Some(entry) => entry.size,
            None => 0,
        };

        // Record deleted entry to trash before tree update
        trash::record_deleted_entry(db, &self.repos, repo_id, path, name, email, &parent_fs_id)
            .await;

        FileOps::update_dir_tree_and_commit(
            db,
            &self.repos,
            repo_id,
            parent_path,
            &parent_fs_id,
            email,
            &format!("Deleted {name}"),
            crate::fs::core::file_ops::EMPTY_ANCESTOR_CHAIN,
            |dirents| {
                dirents.retain(|d| d.name != name);
                Ok(())
            },
        )
        .await
        .map_err(|e| AppError::Internal(e.to_string()))?;

        if let Some(indexer) = &self.indexer
            && let Err(e) = indexer.delete_file(repo_id, path)
        {
            tracing::warn!("Failed to delete index for {path}: {e}");
        }

        crate::fs::core::adjust_repo_size(&self.repos, repo_id, -deleted_size).await?;

        activity_log::log_activity(
            db, repo_id, "delete", "file", path, user_id, None, None, None, None, None,
        )
        .await;

        Ok(())
    }

    /// Rename a file (v2 API form + JSON, plus indexer update).
    pub async fn rename_file(
        &self,
        repo_id: &str,
        path: &str,
        new_name: &str,
        email: &str,
        user_id: i32,
    ) -> Result<(), AppError> {
        rename_entry(
            self.db(),
            &self.repos,
            repo_id,
            path,
            new_name,
            email,
            user_id,
            false,
        )
        .await?;

        if let Some(indexer) = &self.indexer {
            let new_fullpath = if path == "/" || path.is_empty() {
                format!("/{new_name}")
            } else {
                let parent = parent_path_from(path);
                format!("{parent}/{new_name}")
            };
            if let Err(e) = indexer.delete_file(repo_id, path) {
                tracing::warn!("Failed to delete old index on rename: {e}");
            }
            if let Err(e) = indexer
                .reindex_file(repo_id, &new_fullpath, &self.block_store)
                .await
            {
                tracing::warn!("Failed to reindex renamed file: {e}");
            }
        }

        Ok(())
    }

    /// Move a file.
    pub async fn move_file(
        &self,
        repo_id: &str,
        path: &str,
        dst_dir: &str,
        email: &str,
        user_id: i32,
    ) -> Result<(), AppError> {
        let new_path = move_entry(
            self.db(),
            &self.repos,
            repo_id,
            path,
            dst_dir,
            email,
            user_id,
            false,
        )
        .await?;

        // Update full-text search index
        if let Some(indexer) = &self.indexer {
            if let Err(e) = indexer.delete_file(repo_id, path) {
                tracing::warn!("Failed to delete old index on move: {e}");
            }
            if let Err(e) = indexer
                .reindex_file(repo_id, &new_path, &self.block_store)
                .await
            {
                tracing::warn!("Failed to reindex moved file: {e}");
            }
        }

        Ok(())
    }

    /// Get file detail metadata.
    pub async fn file_detail(
        &self,
        repo_id: &str,
        path: &str,
        user_id: i32,
    ) -> Result<serde_json::Value, AppError> {
        let db = self.db();
        let head_root_id = get_head_root_id(db, repo_id).await?;

        let parent_path = parent_path_from(path);
        let file_name = basename(path);

        let parent_fs_id =
            crate::fs::core::resolve_fs_id(&self.repos, repo_id, &head_root_id, parent_path)
                .await
                .map_err(|e| AppError::Internal(format!("resolve parent failed: {e}")))?;
        let parent_data = crate::fs::core::read_fs_dir_data(&self.repos, repo_id, &parent_fs_id)
            .await
            .map_err(|e| AppError::Internal(format!("read parent failed: {e}")))?;
        let entry = parent_data
            .dirents
            .iter()
            .find(|e| e.name == file_name)
            .ok_or_else(|| AppError::NotFound("file not found".into()))?;

        // The entry's mode (from the parent dirent) tells us the type, so the
        // separate resolve_fs_id + fs_object read is unnecessary.
        if entry.mode & S_IFDIR != 0 {
            return Err(AppError::BadRequest(
                "path is a directory, not a file".into(),
            ));
        }
        let file_fs_id = entry.id.clone();

        let modifier_email = self
            .repos
            .user
            .find_by_email(&entry.modifier)
            .await?
            .map(|u| u.email)
            .unwrap_or_else(|| entry.modifier.clone());

        // Read access is guaranteed by the handler; `permission` distinguishes
        // write from read-only (matching seahub's FileDetailView).
        let permission = if crate::domain::permission::check_repo_write_permission(
            self.repos.member.as_ref(),
            repo_id,
            user_id,
        )
        .await
        .is_ok()
        {
            "rw"
        } else {
            "r"
        };

        let starred = self
            .repos
            .starred
            .find_by_user_repo_and_path(user_id, repo_id, path)
            .await?
            .is_some();

        // Absolute avatar URL (seahub returns a full URL via api_avatar_url).
        let origin = self.config.server.site_url_origin();
        let last_modifier_avatar = format!(
            "{}{}",
            origin,
            crate::service::user::primary_avatar_url(&modifier_email, 72)
        );

        Ok(serde_json::json!({
            "id": file_fs_id,
            "type": "file",
            "name": entry.name,
            "permission": permission,
            "mtime": entry.mtime,
            "size": entry.size,
            "last_modified": timestamp_rfc3339(entry.mtime),
            "last_modifier_name": entry.modifier,
            "last_modifier_email": modifier_email,
            "last_modifier_contact_email": modifier_email,
            "last_modifier_avatar": last_modifier_avatar,
            "starred": starred,
            "comment_total": 0,
            "can_edit": permission == "rw",
        }))
    }

    /// Lock or unlock a file.
    pub async fn lock_file(
        &self,
        repo_id: &str,
        path: &str,
        operation: &str,
        email: &str,
    ) -> Result<(), AppError> {
        let user_record = self
            .repos
            .user
            .find_by_email(email)
            .await?
            .ok_or_else(|| AppError::NotFound("user not found".into()))?;

        match operation {
            "lock" => {
                let existing = self
                    .repos
                    .locked_file
                    .find_by_repo_and_path(repo_id, path)
                    .await?;

                if existing.is_none() {
                    self.repos
                        .locked_file
                        .create(
                            repo_id,
                            path,
                            user_record.id,
                            chrono::Utc::now().timestamp(),
                            email,
                        )
                        .await?;
                }

                if let Some(mgr) = &self.notification_manager {
                    let event = FileLockEvent {
                        repo_id: repo_id.to_string(),
                        path: path.to_string(),
                        change_event: "locked".to_string(),
                        lock_user: email.to_string(),
                    };
                    mgr.notify(event).await;
                }
            }
            "unlock" => {
                self.repos
                    .locked_file
                    .delete_by_repo_and_path(repo_id, path)
                    .await?;

                if let Some(mgr) = &self.notification_manager {
                    let event = FileLockEvent {
                        repo_id: repo_id.to_string(),
                        path: path.to_string(),
                        change_event: "unlocked".to_string(),
                        lock_user: email.to_string(),
                    };
                    mgr.notify(event).await;
                }
            }
            _ => {
                return Err(AppError::BadRequest(format!(
                    "unknown operation: {operation}"
                )));
            }
        }

        Ok(())
    }

    /// Lock a file via the sync protocol (includes lock timestamp update).
    pub async fn lock_file_sync(
        &self,
        repo_id: &str,
        path: &str,
        user_id: i32,
    ) -> Result<(), AppError> {
        let now = chrono::Utc::now().timestamp();
        let existing = self
            .repos
            .locked_file
            .find_by_repo_and_path(repo_id, path)
            .await?;

        match existing {
            Some(record) => {
                self.repos
                    .locked_file
                    .update_locked_file_owner(record.id, user_id, now)
                    .await?;
            }
            None => {
                self.repos
                    .locked_file
                    .create(repo_id, path, user_id, now, "")
                    .await?;
            }
        }

        crate::fs::core::upsert_lock_timestamp(self.db.as_ref(), repo_id).await?;
        self.notify_file_lock(repo_id, path, "locked", user_id)
            .await;
        Ok(())
    }

    /// Unlock a file via the sync protocol (includes lock timestamp update).
    pub async fn unlock_file_sync(
        &self,
        repo_id: &str,
        path: &str,
        user_id: i32,
    ) -> Result<(), AppError> {
        self.repos
            .locked_file
            .delete_by_repo_and_path(repo_id, path)
            .await?;

        crate::fs::core::upsert_lock_timestamp(self.db.as_ref(), repo_id).await?;
        self.notify_file_lock(repo_id, path, "unlocked", user_id)
            .await;
        Ok(())
    }

    async fn notify_file_lock(&self, repo_id: &str, path: &str, change_event: &str, user_id: i32) {
        if let Some(mgr) = &self.notification_manager
            && let Ok(Some(user)) = self.repos.user.find_by_id(user_id).await
        {
            let event = FileLockEvent {
                repo_id: repo_id.to_string(),
                path: path.to_string(),
                change_event: change_event.to_string(),
                lock_user: user.email,
            };
            mgr.notify(event).await;
        }
    }

    /// Batch query locked files for a single repo + token pair.
    pub async fn get_locked_files_for_repo(
        &self,
        repo_id: &str,
        token: &str,
    ) -> Result<(Vec<(String, i32)>, i64), AppError> {
        let token_record = self.repos.sync_token.find_by_token(token).await?;
        let token_valid = token_record
            .as_ref()
            .map(|t| t.repo_id == repo_id)
            .unwrap_or(false);
        let token_user_id = token_record.as_ref().map(|t| t.user_id);

        let lock_ts = if token_valid {
            self.repos
                .file_lock_timestamp
                .find_by_repo(repo_id)
                .await?
                .map(|t| t.update_time)
                .unwrap_or(0)
        } else {
            0
        };

        let files = if token_valid {
            let locked = self.repos.locked_file.find_by_repo(repo_id).await?;
            locked
                .into_iter()
                .map(|entry| {
                    let by_me = match token_user_id {
                        Some(tuid) if tuid == entry.user_id => 1,
                        _ => 0,
                    };
                    (entry.path, by_me)
                })
                .collect()
        } else {
            vec![]
        };

        Ok((files, lock_ts))
    }

    /// Create an empty file (v21 API).
    pub async fn create_empty_file(
        &self,
        repo_id: &str,
        path: &str,
        email: &str,
        user_id: i32,
    ) -> Result<(), AppError> {
        let db = self.db();
        let file_name = path.rsplit_once('/').map(|(_, n)| n).unwrap_or(path);
        let parent_path = match path.rsplit_once('/') {
            Some(("", _)) => "/",
            Some((parent, _)) => parent,
            None => "/",
        };

        if file_name.is_empty() {
            return Err(AppError::BadRequest("invalid path".into()));
        }

        let file_fs_data = FsFileData {
            block_ids: vec![],
            size: 0,
            obj_type: 1,
            version: 1,
        };
        let file_fs_id = crate::fs::core::store_fs_file_object(db, repo_id, &file_fs_data).await?;

        let parent_fs_id = if parent_path == "/" {
            match get_head_root_id_opt(db, repo_id).await? {
                Some(root_id) => root_id,
                None => {
                    let empty_dir = FsDirData {
                        dirents: vec![],
                        obj_type: SEAF_METADATA_TYPE_DIR,
                        version: 1,
                    };
                    crate::fs::core::store_fs_dir_object(db, repo_id, &empty_dir).await?
                }
            }
        } else {
            let head_root_id = get_head_root_id_opt(db, repo_id)
                .await?
                .ok_or_else(|| AppError::NotFound("repo has no commits".into()))?;
            crate::fs::core::resolve_fs_id(&self.repos, repo_id, &head_root_id, parent_path)
                .await
                .map_err(|e| AppError::Internal(format!("resolve parent failed: {e}")))?
        };

        let email_clone = email.to_string();
        let file_name_clone = file_name.to_string();
        FileOps::update_dir_tree_and_commit(
            db,
            &self.repos,
            repo_id,
            parent_path,
            &parent_fs_id,
            email,
            &format!("Created empty file {file_name}"),
            crate::fs::core::file_ops::EMPTY_ANCESTOR_CHAIN,
            |dirents| {
                if !dirents.iter().any(|d| d.name == file_name_clone) {
                    dirents.push(DirEntryData {
                        id: file_fs_id.clone(),
                        mode: S_IFREG,
                        modifier: email_clone.clone(),
                        mtime: chrono::Utc::now().timestamp(),
                        name: file_name_clone.clone(),
                        size: 0,
                    });
                }
                Ok(())
            },
        )
        .await
        .map_err(|e| AppError::Internal(e.to_string()))?;

        activity_log::log_activity(
            db, repo_id, "create", "file", path, user_id, None, None, None, None, None,
        )
        .await;

        Ok(())
    }

    /// Check uploaded bytes for resumable upload.
    pub async fn check_uploaded_bytes(&self, blockids: Option<&str>) -> i64 {
        let Some(blockids_str) = blockids else {
            return 0;
        };
        let mut uploaded: i64 = 0;
        for bid in blockids_str.split(',') {
            let bid = bid.trim();
            if !bid.is_empty() && self.block_store.has_block(bid).await {
                uploaded += 1;
            }
        }
        uploaded
    }
}
