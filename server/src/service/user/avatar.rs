use sha2::{Digest, Sha256};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use crate::repository::Repositories;
use base::error::AppError;

/// Default avatar SVG embedded at compile time from `static/img/default-avatar.svg`.
static DEFAULT_AVATAR: &[u8] = include_bytes!("../../../static/img/default-avatar.svg");

pub const MAX_AVATAR_SIZE: usize = 1024 * 1024; // 1 MB
const ALLOWED_EXTENSIONS: &[&str] = &["jpg", "jpeg", "png", "gif"];

/// How long a failed generation is remembered for one size.
///
/// The image route is unauthenticated, so an avatar the worker cannot decode would
/// otherwise start a confined child on every request for it. An hour keeps a broken
/// avatar from being an amplification vector while still retrying reasonably soon.
const AVATAR_FAILURE_TTL: Duration = Duration::from_secs(60 * 60);

pub fn default_avatar_bytes() -> &'static [u8] {
    DEFAULT_AVATAR
}

/// The avatar URL path used across activities, groups and share links.
///
/// The image bytes are a Nanofile addition (seahub serves them from
/// `/media/avatars/…`, a prefix this server does not implement), so the path
/// lives under the `/api/` namespace; the old root spelling answers a permanent
/// redirect.
pub fn primary_avatar_url(email: &str, size: u32) -> String {
    format!("/api/avatars/user/{}/resized/{}/", email, size)
}

pub struct AvatarService {
    repos: Arc<Repositories>,
    avatar_dir: Arc<PathBuf>,
}

impl AvatarService {
    pub fn new(repos: Arc<Repositories>, avatar_dir: Arc<PathBuf>) -> Self {
        Self { repos, avatar_dir }
    }

    /// On-disk directory for a user's avatar files: the first 16 hex characters of the SHA-256 of
    /// the email, matching seafile's `AVATAR_HASH_USERDIRNAMES` behaviour.
    fn avatar_storage_dir(&self, email: &str) -> PathBuf {
        let hash = hex::encode(Sha256::digest(email.as_bytes()));
        self.avatar_dir.join(&hash[..16])
    }

    pub async fn find_avatar(
        &self,
        email: &str,
    ) -> Result<Option<infra::entity::avatar::Model>, AppError> {
        self.repos.avatar.find_by_email(email).await
    }

    /// Validate, store and thumbnail a new avatar; returns its URL.
    pub async fn upload_avatar(
        &self,
        email: &str,
        file_name: String,
        data: Vec<u8>,
    ) -> Result<String, AppError> {
        if data.len() > MAX_AVATAR_SIZE {
            return Err(AppError::BadRequest(format!(
                "avatar file too large (max {} bytes)",
                MAX_AVATAR_SIZE
            )));
        }

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

        let mime_type = match ext.as_str() {
            "jpg" | "jpeg" => "image/jpeg",
            "png" => "image/png",
            "gif" => "image/gif",
            _ => "image/png",
        };

        let storage_dir = self.avatar_storage_dir(email);
        tokio::fs::create_dir_all(&storage_dir)
            .await
            .map_err(|e| AppError::Internal(format!("failed to create avatar dir: {e}")))?;

        // An avatar is a file the account chose, so it is decoded by the sandbox worker. With the
        // sandbox off — or a host below the minimum — there is no other path, and the upload is
        // refused rather than stored for the server to decode later.
        if let Err(why) = crate::sandbox::available(crate::sandbox::Profile::Images) {
            return Err(AppError::BadRequest(format!(
                "avatar processing is unavailable: {why}"
            )));
        }

        // A new original invalidates everything generated from the previous one: the
        // materialised sizes, and any failure marker, which described bytes that no
        // longer exist. Both would otherwise be served (or trusted) for the new
        // avatar.
        clear_generated_files(&storage_dir).await;

        let original_path = storage_dir.join(format!("original.{}", ext));
        tokio::fs::write(&original_path, &data)
            .await
            .map_err(|e| AppError::Internal(format!("failed to save avatar: {e}")))?;

        // The length the record carries, taken before the bytes move into the worker below.
        let data_len = data.len() as i32;

        // Generate and save the 256x256 thumbnail. The decode runs in the confined child, which may
        // work for the whole image timeout, so it queues on the same gate the reading path takes and
        // runs off the runtime — a child spawn blocks. The permit covers that wait.
        if let Ok(_permit) = crate::sandbox::jobs::images::acquire_image_permit().await {
            let worker_input = data;
            let squared = tokio::task::spawn_blocking(move || {
                crate::sandbox::jobs::images::square(&worker_input, 256)
            })
            .await;
            if let Ok(Ok(thumbnail_data)) = squared {
                let thumbnail_path = storage_dir.join("256.png");
                let _ = tokio::fs::write(&thumbnail_path, &thumbnail_data).await;
            }
            // Anything else is non-fatal: the worker may decline a corrupt image, and the original is
            // already saved.
        }

        let now = chrono::Utc::now().timestamp();
        self.repos
            .avatar
            .upsert(email, &file_name, mime_type, data_len, now)
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
        // Only the configured sizes are materialised: the route is reachable
        // without authentication, and a distinct cache entry per integer would
        // let a caller walk the range.
        let size = crate::thumbnail_util::canonical_avatar_size(size);
        let storage_dir = self.avatar_storage_dir(&avatar.email);
        let thumbnail_path = storage_dir.join(format!("{}.png", size));
        let failed_path = storage_dir.join(format!("{}.failed", size));

        if thumbnail_path.exists() {
            return tokio::fs::read(&thumbnail_path)
                .await
                .ok()
                .map(|data| (data, "image/png"));
        }

        // Negative cache: a size the worker could not produce is remembered for an
        // hour, so a broken avatar cannot start a confined child on every request to
        // this unauthenticated route. The original is served instead, exactly as on
        // the failure below.
        if failed_recently(&failed_path).await {
            return original_bytes(&storage_dir).await;
        }

        // A miss starts a confined child that may work for the whole image timeout, so it queues on
        // the same gate the file-thumbnail path uses.
        let _permit = crate::sandbox::jobs::images::acquire_image_permit()
            .await
            .ok()?;

        // Another request may have produced it while this one queued.
        if thumbnail_path.exists() {
            return tokio::fs::read(&thumbnail_path)
                .await
                .ok()
                .map(|data| (data, "image/png"));
        }

        // Thumbnail miss: load the original and have the sandbox worker generate one. The decode is
        // the same hostile-image path as a file thumbnail, in the same confined child.
        let (content, mime) = original_bytes(&storage_dir).await?;

        let worker_input = content.clone();
        let squared = tokio::task::spawn_blocking(move || {
            crate::sandbox::jobs::images::square(&worker_input, size)
        })
        .await
        .ok()?;

        match squared {
            Ok(thumbnail_data) => {
                let _ = tokio::fs::create_dir_all(&storage_dir).await;
                let _ = tokio::fs::write(&thumbnail_path, &thumbnail_data).await;
                // The failure this marker remembered is over.
                let _ = tokio::fs::remove_file(&failed_path).await;
                Some((thumbnail_data, "image/png"))
            }
            Err(_) => {
                // Remember the failure so the next request does not pay for the same
                // child. Non-fatal: the original below is still the answer.
                let _ = tokio::fs::write(&failed_path, b"failed\n").await;
                Some((content, mime))
            }
        }
    }
}

/// Bytes and content type of the uploaded original, if there is one.
async fn original_bytes(storage_dir: &std::path::Path) -> Option<(Vec<u8>, &'static str)> {
    let original_path = find_original_path(storage_dir)?;
    let content = tokio::fs::read(original_path).await.ok()?;
    Some((content, resolve_mime_from_path(storage_dir)))
}

/// Whether a generation failure was recorded for this size recently.
///
/// A marker whose mtime cannot be read, or lies in the future, counts as recent: the
/// alternative is a child per request until the clock catches up, which is the
/// amplification the marker exists to stop.
async fn failed_recently(path: &std::path::Path) -> bool {
    let Ok(metadata) = tokio::fs::metadata(path).await else {
        return false;
    };
    let Ok(modified) = metadata.modified() else {
        return true;
    };
    modified
        .elapsed()
        .map(|age| age < AVATAR_FAILURE_TTL)
        .unwrap_or(true)
}

/// Remove everything in this directory that belongs to the previous avatar: its
/// original (possibly under another extension), the materialised sizes
/// (`{size}.png`) and the failure markers (`{size}.failed`).
///
/// Called before a new original is written, so nothing derived from the previous
/// avatar outlives it — including an `original.png` that would otherwise shadow a
/// new `original.jpg` in [`find_original_path`].
async fn clear_generated_files(storage_dir: &std::path::Path) {
    let Ok(mut entries) = tokio::fs::read_dir(storage_dir).await else {
        return;
    };
    while let Ok(Some(entry)) = entries.next_entry().await {
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        if name.starts_with("original.") || name.ends_with(".png") || name.ends_with(".failed") {
            let _ = tokio::fs::remove_file(entry.path()).await;
        }
    }
}

/// Parse the `{size}` parameter — accept a number or the literal `"default"`.
pub fn resolve_size(size_str: &str) -> u32 {
    if size_str.eq_ignore_ascii_case("default") {
        256
    } else {
        size_str.parse::<u32>().unwrap_or(256)
    }
}

/// Find the original uploaded file in a storage directory, trying known image extensions in order.
pub(crate) fn find_original_path(dir: &std::path::Path) -> Option<PathBuf> {
    for ext in &["png", "jpg", "jpeg", "gif"] {
        let path = dir.join(format!("original.{}", ext));
        if path.exists() {
            return Some(path);
        }
    }
    None
}

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
