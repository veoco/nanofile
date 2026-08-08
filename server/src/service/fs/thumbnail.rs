use futures::StreamExt;
use sha2::{Digest, Sha256};
use std::path::PathBuf;
use std::process::Command;
use std::sync::Arc;
use std::sync::OnceLock;
use tokio::io::AsyncWriteExt;

use crate::fs::core::download::Downloader;
use crate::fs::core::tree::{read_fs_dir_data, resolve_fs_id};
use crate::repository::Repositories;
use base::common::{EMPTY_SHA1, SEAF_METADATA_TYPE_DIR};
use base::error::AppError;

pub struct ThumbnailService {
    repos: Arc<Repositories>,
    block_store: infra::storage::DynBlockStorage,
    block_dir: Arc<PathBuf>,
    /// Scratch directory for streaming video files before ffmpeg extracts a frame.
    temp_dir: Arc<PathBuf>,
    /// Path to the `ffmpeg` binary ("ffmpeg" by default).
    ffmpeg_path: Arc<String>,
}

impl ThumbnailService {
    pub fn new(
        repos: Arc<Repositories>,
        block_store: infra::storage::DynBlockStorage,
        block_dir: Arc<PathBuf>,
        temp_dir: Arc<PathBuf>,
        ffmpeg_path: Arc<String>,
    ) -> Self {
        Self {
            repos,
            block_store,
            block_dir,
            temp_dir,
            ffmpeg_path,
        }
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

        let file_fs_id =
            resolve_fs_id(&self.repos, repo_id, &head_commit.root_id, &normalized_path)
                .await
                .map_err(|_| AppError::NotFound("file not found".into()))?;

        if file_fs_id == EMPTY_SHA1 {
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

        // Supported sources are images (decoded in-process) and audio/video
        // (a frame or embedded cover art extracted via ffmpeg). Anything else
        // has no thumbnail.
        let ext = std::path::Path::new(&file_name)
            .extension()
            .and_then(|e| e.to_str())
            .map(|e| e.to_lowercase())
            .unwrap_or_default();
        let is_image = crate::thumbnail_util::is_supported_image_ext(&ext);
        let is_video = crate::thumbnail_util::is_video_ext(&ext);
        let is_audio = crate::thumbnail_util::is_audio_ext(&ext);
        if !is_image && !is_video && !is_audio {
            return Err(AppError::NotFound("thumbnail not available".into()));
        }

        let thumbnail_data = if is_image {
            // Skip huge images and cap the in-memory read; decoding a truncated
            // multi-hundred-MB image would waste CPU and memory for no benefit.
            const MAX_THUMBNAIL_SOURCE: i64 = 32 * 1024 * 1024;
            let (file_data, _block_ids) =
                Downloader::resolve_blocks(&self.repos, repo_id, &normalized_path)
                    .await
                    .map_err(|_| AppError::NotFound("thumbnail not available".into()))?;
            if file_data.size > MAX_THUMBNAIL_SOURCE {
                return Err(AppError::NotFound("thumbnail not available".into()));
            }

            let content = Downloader::download_file_limited(
                &self.repos,
                repo_id,
                &normalized_path,
                &self.block_store,
                None,
                MAX_THUMBNAIL_SOURCE as usize,
            )
            .await
            .map_err(|_| AppError::NotFound("thumbnail not available".into()))?;

            tokio::task::spawn_blocking(move || {
                crate::thumbnail_util::generate_thumbnail(&content, size)
            })
            .await
            .map_err(|e| AppError::Internal(format!("thumbnail generation panicked: {e}")))?
            .map_err(|e| AppError::Internal(format!("thumbnail generation failed: {e}")))?
        } else {
            let kind = if is_video {
                MediaKind::Video
            } else {
                MediaKind::Audio
            };
            self.generate_media_thumbnail(repo_id, &normalized_path, size, kind)
                .await?
        };

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

    /// Generate a thumbnail for an audio/video file via ffmpeg.
    ///
    /// The file is streamed to a scratch file under `temp_dir` so ffmpeg can
    /// seek. For video a frame is captured ~1s in (falling back to the first
    /// frame); for audio the embedded cover art is extracted. The result is
    /// fitted/resized to `size` and re-encoded as PNG by the shared image util.
    /// Returns `NotFound` when ffmpeg is unavailable or no frame/cover exists —
    /// the UI then falls back to an extension badge / play icon.
    async fn generate_media_thumbnail(
        &self,
        repo_id: &str,
        normalized_path: &str,
        size: u32,
        kind: MediaKind,
    ) -> Result<Vec<u8>, AppError> {
        if !ffmpeg_available(&self.ffmpeg_path) {
            return Err(AppError::NotFound("thumbnail not available".into()));
        }

        // Skip absurdly large sources — extracting a frame would stream gigabytes.
        const MAX_VIDEO_SOURCE: i64 = 2 * 1024 * 1024 * 1024;
        let (file_data, block_ids) =
            Downloader::resolve_blocks(&self.repos, repo_id, normalized_path)
                .await
                .map_err(|_| AppError::NotFound("thumbnail not available".into()))?;
        if file_data.size > MAX_VIDEO_SOURCE {
            return Err(AppError::NotFound("thumbnail not available".into()));
        }

        // Stream the whole media file to a scratch file so ffmpeg can seek.
        let scratch_dir = self.temp_dir.join("media_thumbs");
        tokio::fs::create_dir_all(&scratch_dir)
            .await
            .map_err(|e| AppError::Internal(format!("create scratch dir failed: {e}")))?;
        let scratch_media = scratch_dir.join(format!(
            "{}_{}.bin",
            repo_id,
            thumbnail_key(repo_id, normalized_path)
        ));
        let scratch_png = scratch_media.with_extension("png");

        let write_result: Result<(), std::io::Error> = async {
            let mut out = tokio::fs::File::create(&scratch_media).await?;
            let mut stream =
                crate::fs::core::download::stream_blocks(block_ids, self.block_store.clone(), None);
            while let Some(chunk) = stream.next().await {
                out.write_all(&chunk?).await?;
            }
            out.flush().await
        }
        .await;
        if write_result.is_err() {
            let _ = tokio::fs::remove_file(&scratch_media).await;
            return Err(AppError::NotFound("thumbnail not available".into()));
        }

        let ffmpeg = self.ffmpeg_path.to_string();
        let src = scratch_media.clone();
        let dst = scratch_png.clone();
        let extracted =
            tokio::task::spawn_blocking(move || extract_media_frame(&ffmpeg, kind, &src, &dst))
                .await
                .map_err(|e| AppError::Internal(format!("ffmpeg panicked: {e}")))?;

        let _ = tokio::fs::remove_file(&scratch_media).await;
        if !extracted {
            let _ = tokio::fs::remove_file(&scratch_png).await;
            return Err(AppError::NotFound("thumbnail not available".into()));
        }

        let png = tokio::fs::read(&scratch_png)
            .await
            .map_err(|e| AppError::Internal(e.to_string()))?;
        let _ = tokio::fs::remove_file(&scratch_png).await;

        tokio::task::spawn_blocking(move || crate::thumbnail_util::generate_thumbnail(&png, size))
            .await
            .map_err(|e| AppError::Internal(format!("thumbnail generation panicked: {e}")))?
            .map_err(|e| AppError::Internal(format!("thumbnail generation failed: {e}")))
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

        let parent_fs_id = resolve_fs_id(&self.repos, repo_id, root_fs_id, parent_path)
            .await
            .map_err(|_| AppError::NotFound("parent path not found".into()))?;

        let dir_data = read_fs_dir_data(&self.repos, repo_id, &parent_fs_id)
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

/// What kind of media a ffmpeg-based thumbnail should extract.
#[derive(Clone, Copy)]
enum MediaKind {
    /// A frame from the video stream (~1s in, first frame as fallback).
    Video,
    /// The embedded cover art (attached picture) of an audio file.
    Audio,
}

/// Whether the configured ffmpeg binary exists and runs. Cached for the
/// process lifetime (the path comes from config and doesn't change at runtime).
fn ffmpeg_available(ffmpeg: &str) -> bool {
    static AVAILABLE: OnceLock<bool> = OnceLock::new();
    *AVAILABLE.get_or_init(|| {
        Command::new(ffmpeg)
            .arg("-version")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    })
}

/// Run ffmpeg to extract one image from `src` into `dst` (a PNG).
///
/// - `Video`: grabs a frame ~1s in (skips dark intros), falling back to the
///   first frame on failure.
/// - `Audio`: extracts the embedded cover art via `-map 0:v:0` (no seek).
///
/// Returns true when an image was written.
fn extract_media_frame(
    ffmpeg: &str,
    kind: MediaKind,
    src: &std::path::Path,
    dst: &std::path::Path,
) -> bool {
    let mut attempts: Vec<Option<&str>> = vec![None];
    if matches!(kind, MediaKind::Video) {
        attempts = vec![Some("1"), None];
    }
    for ss in attempts {
        let mut cmd = Command::new(ffmpeg);
        cmd.arg("-y")
            .arg("-loglevel")
            .arg("error")
            .arg("-hide_banner");
        if let Some(ss) = ss {
            cmd.arg("-ss").arg(ss);
        }
        cmd.arg("-i").arg(src);
        if matches!(kind, MediaKind::Audio) {
            cmd.arg("-map").arg("0:v:0");
        }
        cmd.arg("-frames:v")
            .arg("1")
            .arg("-vf")
            .arg("scale=min(1024\\,iw):-2")
            .arg("-f")
            .arg("image2")
            .arg(dst)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null());
        if cmd.status().map(|s| s.success()).unwrap_or(false) {
            return true;
        }
    }
    false
}

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

#[cfg(test)]
mod tests {
    use super::*;

    /// Extract a frame from a synthetic ffmpeg-generated video and verify the
    /// output is a decodable PNG that the shared thumbnail fitter accepts.
    /// Skips silently when ffmpeg isn't installed on the host (CI installs it).
    #[test]
    fn extract_video_frame_writes_png() {
        if !ffmpeg_available("ffmpeg") {
            eprintln!("ffmpeg not available; skipping video thumbnail test");
            return;
        }

        let dir = std::env::temp_dir().join(format!(
            "nanofile_vthumb_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let src = dir.join("test.mp4");
        let dst = dir.join("frame.png");

        // Generate a small synthetic video (2s of test pattern).
        let status = Command::new("ffmpeg")
            .arg("-y")
            .arg("-loglevel")
            .arg("error")
            .arg("-f")
            .arg("lavfi")
            .arg("-i")
            .arg("testsrc=duration=2:size=320x240:rate=10")
            .arg("-pix_fmt")
            .arg("yuv420p")
            .arg(&src)
            .status()
            .expect("failed to spawn ffmpeg");
        assert!(status.success(), "ffmpeg failed to create test video");

        let ok = extract_media_frame("ffmpeg", MediaKind::Video, &src, &dst);
        let _ = std::fs::remove_file(&src);
        assert!(ok, "ffmpeg frame extraction failed");

        let bytes = std::fs::read(&dst).unwrap();
        let _ = std::fs::remove_file(&dst);
        let decoded =
            image::load_from_memory(&bytes).expect("extracted frame is not a valid image");
        assert!(decoded.width() > 0 && decoded.height() > 0);

        let thumb = crate::thumbnail_util::generate_thumbnail(&bytes, 48)
            .expect("thumbnail fitter failed on extracted frame");
        assert!(!thumb.is_empty());
    }

    /// Build an m4a with embedded cover art and verify `extract_media_frame`
    /// (Audio) pulls the cover out as a decodable PNG. Skips without ffmpeg.
    #[test]
    fn extract_audio_cover_writes_png() {
        if !ffmpeg_available("ffmpeg") {
            eprintln!("ffmpeg not available; skipping audio cover test");
            return;
        }

        let dir = std::env::temp_dir().join(format!(
            "nanofile_athumb_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let src = dir.join("cover.m4a");
        let dst = dir.join("cover.png");

        // 1s sine tone as audio + a 1-frame pattern as attached cover art.
        let status = Command::new("ffmpeg")
            .arg("-y")
            .arg("-loglevel")
            .arg("error")
            .arg("-f")
            .arg("lavfi")
            .arg("-i")
            .arg("sine=frequency=440:duration=1")
            .arg("-f")
            .arg("lavfi")
            .arg("-i")
            .arg("testsrc=size=64x64:rate=1:duration=1")
            .arg("-map")
            .arg("0:a")
            .arg("-map")
            .arg("1:v")
            .arg("-c:a")
            .arg("aac")
            .arg("-c:v")
            .arg("mjpeg")
            .arg("-disposition:v")
            .arg("attached_pic")
            .arg("-shortest")
            .arg(&src)
            .status()
            .expect("failed to spawn ffmpeg");
        assert!(status.success(), "ffmpeg failed to create test audio");

        let ok = extract_media_frame("ffmpeg", MediaKind::Audio, &src, &dst);
        let _ = std::fs::remove_file(&src);
        assert!(ok, "audio cover extraction failed");

        let bytes = std::fs::read(&dst).unwrap();
        let _ = std::fs::remove_file(&dst);
        let decoded =
            image::load_from_memory(&bytes).expect("extracted cover is not a valid image");
        assert!(decoded.width() > 0 && decoded.height() > 0);

        let thumb = crate::thumbnail_util::generate_thumbnail(&bytes, 48)
            .expect("thumbnail fitter failed on extracted cover");
        assert!(!thumb.is_empty());
    }
}
