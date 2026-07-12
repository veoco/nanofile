use sha2::{Digest, Sha256};
use std::path::PathBuf;
use std::sync::Arc;

use crate::repository::Repositories;
use base::error::AppError;

// ─── Constants ───────────────────────────────────────────────────────────────

/// Default avatar SVG embedded at compile time.
/// Path relative to this file: ../../../static/img/default-avatar.svg
static DEFAULT_AVATAR: &[u8] = include_bytes!("../../../static/img/default-avatar.svg");

const MAX_AVATAR_SIZE: usize = 1024 * 1024; // 1 MB
const ALLOWED_EXTENSIONS: &[&str] = &["jpg", "jpeg", "png", "gif"];

// ─── Public utility functions ────────────────────────────────────────────────

/// Return the embedded default-avatar bytes.
pub fn default_avatar_bytes() -> &'static [u8] {
    DEFAULT_AVATAR
}

/// Compute the on-disk storage directory for a user's avatar files.
///
/// Uses the first 16 hex characters of the SHA-256 of the user's email as the
/// directory name, matching seafile's `AVATAR_HASH_USERDIRNAMES` behaviour.
pub fn avatar_storage_dir(email: &str) -> PathBuf {
    let hash = hex::encode(Sha256::digest(email.as_bytes()));
    PathBuf::from("data/avatars").join(&hash[..16])
}

/// Build the avatar URL path used by the rest of the codebase
/// (activities, groups, share links, etc.).
pub fn primary_avatar_url(email: &str, size: u32) -> String {
    format!("/avatars/user/{}/resized/{}/", email, size)
}

// ─── Service ─────────────────────────────────────────────────────────────────

pub struct AvatarService {
    repos: Arc<Repositories>,
}

impl AvatarService {
    pub fn new(repos: Arc<Repositories>) -> Self {
        Self { repos }
    }

    /// Find an avatar record by email.
    pub async fn find_avatar(
        &self,
        email: &str,
    ) -> Result<Option<infra::entity::avatar::Model>, AppError> {
        self.repos.avatar.find_by_email(email).await
    }

    /// Upload a new avatar for the user. Validates file size and extension,
    /// persists the original to disk, generates a 256x256 thumbnail, and
    /// upserts the database record. Returns the avatar URL.
    pub async fn upload_avatar(
        &self,
        email: &str,
        file_name: String,
        data: Vec<u8>,
    ) -> Result<String, AppError> {
        // Validate file size
        if data.len() > MAX_AVATAR_SIZE {
            return Err(AppError::BadRequest(format!(
                "avatar file too large (max {} bytes)",
                MAX_AVATAR_SIZE
            )));
        }

        // Validate file extension
        let ext = file_name
            .rsplit_once('.')
            .map(|(_, e)| e.to_lowercase())
            .ok_or_else(|| AppError::BadRequest("file has no extension".into()))?;

        if !ALLOWED_EXTENSIONS.contains(&ext.as_str()) {
            return Err(AppError::BadRequest(format!(
                "invalid file extension: .{ext} (allowed: {:?})",
                ALLOWED_EXTENSIONS
            )));
        }

        // Determine mime type from extension
        let mime_type = match ext.as_str() {
            "jpg" | "jpeg" => "image/jpeg",
            "png" => "image/png",
            "gif" => "image/gif",
            _ => "image/png",
        };

        // Build storage path
        let storage_dir = avatar_storage_dir(email);
        tokio::fs::create_dir_all(&storage_dir)
            .await
            .map_err(|e| AppError::Internal(format!("failed to create avatar dir: {e}")))?;

        // Save the original file
        let original_path = storage_dir.join(format!("original.{}", ext));
        tokio::fs::write(&original_path, &data)
            .await
            .map_err(|e| AppError::Internal(format!("failed to save avatar: {e}")))?;

        // Generate and save the default-size thumbnail (256x256)
        match crate::thumbnail_util::generate_square_thumbnail(&data, 256) {
            Ok(thumbnail_data) => {
                let thumbnail_path = storage_dir.join("256.png");
                let _ = tokio::fs::write(&thumbnail_path, &thumbnail_data).await;
            }
            Err(_) => {
                // Non-fatal: thumbnail generation may fail for corrupt images,
                // but the original was already saved
            }
        }

        // Upsert avatar database record
        let now = chrono::Utc::now().timestamp();
        self.repos
            .avatar
            .upsert(email, &file_name, mime_type, data.len() as i32, now)
            .await?;

        Ok(primary_avatar_url(email, 256))
    }

    /// Get the avatar bytes for serving. Returns `(bytes, mime_type)` for an
    /// existing avatar at the requested thumbnail size, or `None` for default.
    pub async fn read_avatar_bytes(
        &self,
        avatar: &infra::entity::avatar::Model,
        size: u32,
    ) -> Option<(Vec<u8>, &'static str)> {
        let storage_dir = avatar_storage_dir(&avatar.email);
        let thumbnail_path = storage_dir.join(format!("{}.png", size));

        // Fast path — thumbnail already cached on disk
        if thumbnail_path.exists() {
            return tokio::fs::read(&thumbnail_path)
                .await
                .ok()
                .map(|data| (data, "image/png"));
        }

        // Thumbnail miss — load the original and generate one
        let original_path = find_original_path(&storage_dir)?;
        let content = std::fs::read(original_path).ok()?;

        match crate::thumbnail_util::generate_square_thumbnail(&content, size) {
            Ok(thumbnail_data) => {
                // Persist for future requests (non-fatal if it fails)
                let _ = tokio::fs::create_dir_all(&storage_dir).await;
                let _ = tokio::fs::write(&thumbnail_path, &thumbnail_data).await;
                Some((thumbnail_data, "image/png"))
            }
            Err(_) => {
                // Fall back to serving the original at its native size
                let mime = resolve_mime_from_path(&storage_dir);
                Some((content, mime))
            }
        }
    }
}

// ─── Internal helpers ────────────────────────────────────────────────────────

/// Parse the `{size}` parameter — accept a number or the literal `"default"`.
pub fn resolve_size(size_str: &str) -> u32 {
    if size_str.eq_ignore_ascii_case("default") {
        256
    } else {
        size_str.parse::<u32>().unwrap_or(256)
    }
}

/// Find the original uploaded file inside a storage directory by trying known
/// image extensions in order.
pub(crate) fn find_original_path(dir: &std::path::Path) -> Option<PathBuf> {
    for ext in &["png", "jpg", "jpeg", "gif"] {
        let path = dir.join(format!("original.{}", ext));
        if path.exists() {
            return Some(path);
        }
    }
    None
}

/// Guess the MIME type from the original file extension in a storage directory.
fn resolve_mime_from_path(dir: &std::path::Path) -> &'static str {
    for ext in &["png", "jpg", "jpeg", "gif"] {
        if dir.join(format!("original.{}", ext)).exists() {
            return match *ext {
                "png" => "image/png",
                "jpg" | "jpeg" => "image/jpeg",
                "gif" => "image/gif",
                _ => "image/png",
            };
        }
    }
    "image/png"
}
