use sha2::{Digest, Sha256};
use std::path::PathBuf;
use std::sync::Arc;

use sea_orm::DatabaseConnection;

use crate::error::AppError;
use crate::repo::download::Downloader;
use crate::repo::fs_tree::{read_fs_dir_data, resolve_fs_id};
use crate::repository::Repositories;
use crate::serialization::fs_json::SEAF_METADATA_TYPE_DIR;

pub struct ThumbnailService {
    repos: Arc<Repositories>,
    db: Arc<DatabaseConnection>,
    block_store: crate::storage::DynBlockStorage,
    block_dir: Arc<PathBuf>,
}

impl ThumbnailService {
    pub fn new(
        repos: Arc<Repositories>,
        db: Arc<DatabaseConnection>,
        block_store: crate::storage::DynBlockStorage,
        block_dir: Arc<PathBuf>,
    ) -> Self {
        Self {
            repos,
            db,
            block_store,
            block_dir,
        }
    }

    fn db(&self) -> &DatabaseConnection {
        self.db.as_ref()
    }

    /// Path to the repo-level thumbnail cache directory.
    fn thumbnail_repo_dir(&self, repo_id: &str) -> PathBuf {
        self.block_dir
            .parent()
            .unwrap_or(&self.block_dir)
            .join("thumbnails")
            .join(repo_id)
    }

    /// Deterministic on-disk filename for a thumbnail, matching seahub's
    /// `generate_thumbnail_key()` approach but using MD5(repo_id + path)
    /// instead of a bare path (avoids path-collision bugs).
    fn thumbnail_file_path(&self, repo_id: &str, path: &str, size: u32) -> PathBuf {
        let hash = thumbnail_key(repo_id, path);
        self.thumbnail_repo_dir(repo_id)
            .join(format!("{hash}_{size}.png"))
    }

    /// Get or generate a thumbnail for a file.
    ///
    /// Returns the PNG thumbnail data.
    pub async fn get_thumbnail(
        &self,
        repo_id: &str,
        path: &str,
        size: u32,
    ) -> Result<Vec<u8>, AppError> {
        let normalized_path = if path.is_empty() || path == "/" {
            "/".to_string()
        } else if path.starts_with('/') {
            path.to_string()
        } else {
            format!("/{path}")
        };

        // Verify path exists and is a file
        let repo_model = self
            .repos
            .repo
            .find_by_id(repo_id)
            .await?
            .ok_or_else(|| AppError::NotFound("Repository not found".into()))?;
        let head_commit_id = repo_model
            .head_commit_id
            .ok_or_else(|| AppError::NotFound("No commits yet".into()))?;
        let head_commit = self
            .repos
            .commit
            .find_by_id(&head_commit_id)
            .await?
            .ok_or_else(|| AppError::NotFound("Head commit not found".into()))?;

        let file_fs_id = resolve_fs_id(self.db(), repo_id, &head_commit.root_id, &normalized_path)
            .await
            .map_err(|_| AppError::NotFound("file not found".into()))?;

        if file_fs_id == "0000000000000000000000000000000000000000" {
            return Err(AppError::BadRequest("path is a directory".into()));
        }

        let file_obj = self
            .repos
            .fs_object
            .find_by_repo_and_fs_id(repo_id, &file_fs_id)
            .await?
            .ok_or_else(|| AppError::NotFound("file not found".into()))?;

        if file_obj.obj_type == SEAF_METADATA_TYPE_DIR as i8 {
            return Err(AppError::BadRequest("path is a directory".into()));
        }

        let file_name = normalized_path
            .rsplit_once('/')
            .map(|(_, n)| n)
            .unwrap_or("file")
            .to_string();

        // ── Get the file's current modification time from the parent dir ──
        let current_mtime = self
            .resolve_file_mtime(repo_id, &head_commit.root_id, &normalized_path)
            .await?;

        // ── Check if a valid cached thumbnail exists ──
        let thumbnail_path = self.thumbnail_file_path(repo_id, &normalized_path, size);
        let existing = self
            .repos
            .thumbnail
            .find_by_repo_path_size(repo_id, &normalized_path, size as i32)
            .await?;

        if let Some(record) = existing {
            // Staleness check: if source file was modified after the thumbnail was created, regenerate
            if record.file_modified_at >= current_mtime && thumbnail_path.exists() {
                return tokio::fs::read(&thumbnail_path)
                    .await
                    .map_err(|e| AppError::Internal(e.to_string()));
            }
            // Stale — fall through to regenerate
        }

        // Check if this is a supported image file
        let ext = std::path::Path::new(&file_name)
            .extension()
            .and_then(|e| e.to_str())
            .map(|e| e.to_lowercase())
            .unwrap_or_default();

        if !crate::thumbnail_util::is_supported_image_ext(&ext) {
            return Err(AppError::NotFound("thumbnail not available".into()));
        }

        // Download file content and generate thumbnail
        let content = Downloader::download_file(
            &self.repos,
            self.db(),
            repo_id,
            &normalized_path,
            &self.block_store,
            None,
        )
        .await
        .map_err(|_| AppError::NotFound("thumbnail not available".into()))?;

        let thumbnail_data = tokio::task::spawn_blocking(move || {
            crate::thumbnail_util::generate_thumbnail(&content, size)
        })
        .await
        .map_err(|e| AppError::Internal(format!("thumbnail generation panicked: {e}")))?
        .map_err(|e| AppError::Internal(format!("thumbnail generation failed: {e}")))?;

        // ── Store thumbnail for future requests ──
        let thumbnail_dir = self.thumbnail_repo_dir(repo_id);
        tokio::fs::create_dir_all(&thumbnail_dir).await?;
        let _ = tokio::fs::write(&thumbnail_path, &thumbnail_data).await;

        // ── Upsert database record (if stale, update; if new, insert) ──
        let now = chrono::Utc::now().timestamp();
        if let Some(_record) = self
            .repos
            .thumbnail
            .find_by_repo_path_size(repo_id, &normalized_path, size as i32)
            .await?
        {
            self.repos
                .thumbnail
                .update_mtime(repo_id, &normalized_path, size as i32, current_mtime, now)
                .await?;
            // Delete old-naming disk file if it still exists (migration from old path scheme)
            let legacy_path = self
                .block_dir
                .parent()
                .unwrap_or(&self.block_dir)
                .join("thumbnails")
                .join(repo_id)
                .join(format!(
                    "{}_{}.png",
                    normalize_path_for_file(&normalized_path),
                    size
                ));
            let _ = tokio::fs::remove_file(&legacy_path).await;
        } else {
            self.repos
                .thumbnail
                .create(repo_id, &normalized_path, size as i32, current_mtime, now)
                .await?;
        }

        Ok(thumbnail_data)
    }

    /// Remove all cached thumbnails (disk + DB) for a given repo path.
    /// Called when a file is deleted.
    pub async fn cleanup(&self, repo_id: &str, path: &str) {
        let normalized = if path.is_empty() || path == "/" {
            "/"
        } else if path.starts_with('/') {
            path
        } else {
            return; // non-absolute paths shouldn't happen
        };

        // 1. Delete DB records
        let _ = self
            .repos
            .thumbnail
            .delete_by_path(repo_id, normalized)
            .await;

        // 2. Delete disk files by enumerating the repo thumbnail dir
        let dir = self.thumbnail_repo_dir(repo_id);
        let prefix = thumbnail_key(repo_id, normalized);
        if let Ok(mut entries) = tokio::fs::read_dir(&dir).await {
            while let Ok(Some(entry)) = entries.next_entry().await {
                if let Some(name) = entry.file_name().to_str()
                    && name.starts_with(&prefix)
                {
                    let _ = tokio::fs::remove_file(entry.path()).await;
                }
            }
        }
    }

    /// Resolve the current `mtime` for a file by reading its parent directory
    /// entry.
    async fn resolve_file_mtime(
        &self,
        repo_id: &str,
        root_fs_id: &str,
        path: &str,
    ) -> Result<i64, AppError> {
        let (parent_path, file_name) = path
            .rsplit_once('/')
            .map(|(p, n)| (if p.is_empty() { "/" } else { p }, n))
            .unwrap_or(("/", ""));

        let parent_fs_id = resolve_fs_id(self.db(), repo_id, root_fs_id, parent_path)
            .await
            .map_err(|_| AppError::NotFound("parent path not found".into()))?;

        let dir_data = read_fs_dir_data(self.db(), repo_id, &parent_fs_id)
            .await
            .map_err(|e| AppError::Internal(format!("failed to read parent dir: {e}")))?;

        dir_data
            .dirents
            .iter()
            .find(|d| d.name == file_name)
            .map(|d| d.mtime)
            .ok_or_else(|| AppError::NotFound("file entry not found in parent dir".into()))
    }
}

// ─── Helpers ──────────────────────────────────────────────────────────────

/// Build a deterministic, collision-free filename prefix for a thumbnail.
/// Uses SHA256(repo_id + path) — matching seahub's `generate_thumbnail_key()` approach
/// but with SHA-256 instead of MD5 (seahub uses MD5, but SHA-256 is already a dependency).
fn thumbnail_key(repo_id: &str, path: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(repo_id.as_bytes());
    hasher.update(path.as_bytes());
    let hash = hex::encode(hasher.finalize());
    // Use first 32 hex chars (128 bits) — plenty for collision avoidance
    format!("thumb_{}", &hash[..32])
}

/// Old path-normalization function kept only for migration cleanup.
/// Replaced by `thumbnail_key()` which avoids path collisions.
fn normalize_path_for_file(path: &str) -> String {
    path.trim_matches('/').replace('/', "_")
}
