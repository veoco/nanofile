use std::path::PathBuf;
use std::sync::Arc;

use sea_orm::DatabaseConnection;

use crate::error::AppError;
use crate::repo::download::Downloader;
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

    /// Get or generate a thumbnail for a file.
    ///
    /// Returns the PNG thumbnail data and whether it was newly generated.
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

        let file_fs_id =
            crate::repo::resolve_fs_id(self.db(), repo_id, &head_commit.root_id, &normalized_path)
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

        // Check if thumbnail already exists in cache
        let existing = self
            .repos
            .thumbnail
            .find_by_repo_path_size(repo_id, &normalized_path, size as i32)
            .await?;

        if existing.is_some() {
            let thumbnail_path = self
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
            if thumbnail_path.exists() {
                return tokio::fs::read(&thumbnail_path)
                    .await
                    .map_err(|e| AppError::Internal(e.to_string()));
            }
        }

        // Check if this is an image file
        let ext = std::path::Path::new(&file_name)
            .extension()
            .and_then(|e| e.to_str())
            .map(|e| e.to_lowercase())
            .unwrap_or_default();

        let is_supported = matches!(
            ext.as_str(),
            "png" | "jpg" | "jpeg" | "gif" | "bmp" | "webp"
        );
        if !is_supported {
            return Err(AppError::NotFound("thumbnail not available".into()));
        }

        // Download file content and generate thumbnail
        let content = Downloader::download_file(
            self.db(),
            repo_id,
            &normalized_path,
            &self.block_store,
            None,
        )
        .await
        .map_err(|_| AppError::NotFound("thumbnail not available".into()))?;

        let thumbnail_data = generate_thumbnail(&content, size)?;

        // Store thumbnail for future requests
        let thumbnail_dir = self
            .block_dir
            .parent()
            .unwrap_or(&self.block_dir)
            .join("thumbnails")
            .join(repo_id);
        tokio::fs::create_dir_all(&thumbnail_dir).await?;
        let thumbnail_file = thumbnail_dir.join(format!(
            "{}_{}.png",
            normalize_path_for_file(&normalized_path),
            size
        ));
        tokio::fs::write(&thumbnail_file, &thumbnail_data).await?;

        // Record in database
        let now = chrono::Utc::now().timestamp();
        self.repos
            .thumbnail
            .create(repo_id, &normalized_path, size as i32, now)
            .await?;

        Ok(thumbnail_data)
    }
}

/// Generate a thumbnail image from raw file content.
fn generate_thumbnail(content: &[u8], size: u32) -> Result<Vec<u8>, AppError> {
    let img = image::load_from_memory(content)
        .map_err(|_| AppError::NotFound("unable to decode image".into()))?;

    let thumbnail = img.thumbnail(size, size);
    let mut output = Vec::new();
    thumbnail
        .write_to(
            &mut std::io::Cursor::new(&mut output),
            image::ImageFormat::Png,
        )
        .map_err(|e| AppError::Internal(e.to_string()))?;

    Ok(output)
}

/// Replace path separators with underscores for safe filenames.
fn normalize_path_for_file(path: &str) -> String {
    path.trim_matches('/').replace('/', "_")
}
