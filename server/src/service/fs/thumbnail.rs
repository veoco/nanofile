use futures::StreamExt;
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
#[cfg(test)]
use std::process::Command;
use std::sync::Arc;
use tokio::io::AsyncWriteExt;

use crate::fs::core::download::Downloader;
use crate::fs::core::tree::resolve_file_entry;
use crate::repository::Repositories;
use crate::sandbox::jobs::images;
use crate::sandbox::jobs::media::{self, Kind as MediaKind};
use crate::thumbnail_util::ThumbFormat;
use base::common::{EMPTY_SHA1, FsFileData, SEAF_METADATA_TYPE_DIR};
use base::error::AppError;

/// Acquire a permit covering one thumbnail generation.
///
/// The gate is shared with the image worker so every path that starts an image child is paced by
/// one bound: a cache miss decodes a large image or spawns an `ffmpeg` child that may read a
/// 512 MiB file. Callers queue rather than fail, preserving the behaviour of a normal burst.
async fn acquire_thumbnail_permit() -> Result<tokio::sync::OwnedSemaphorePermit, AppError> {
    crate::sandbox::jobs::images::acquire_image_permit()
        .await
        .map_err(AppError::Internal)
}

pub struct ThumbnailService {
    repos: Arc<Repositories>,
    block_store: infra::storage::DynBlockStorage,
    thumbnail_dir: Arc<PathBuf>,
    temp_dir: Arc<PathBuf>,
    /// Path to the `ffmpeg` binary ("ffmpeg" by default).
    ffmpeg_path: Arc<String>,
}

impl ThumbnailService {
    pub fn new(
        repos: Arc<Repositories>,
        block_store: infra::storage::DynBlockStorage,
        thumbnail_dir: Arc<PathBuf>,
        temp_dir: Arc<PathBuf>,
        ffmpeg_path: Arc<String>,
    ) -> Self {
        Self {
            repos,
            block_store,
            thumbnail_dir,
            temp_dir,
            ffmpeg_path,
        }
    }

    /// Path to the repo-level thumbnail cache directory.
    ///
    /// The segment is `sha1(repo_id)`, never the raw id: that id is client-supplied and must not
    /// escape `thumbnail_dir` via `..` or an absolute path.
    fn thumbnail_repo_dir(&self, repo_id: &str) -> PathBuf {
        self.thumbnail_dir.join(thumbnail_dir_name(repo_id))
    }

    /// Deterministic on-disk filename for a thumbnail.
    ///
    /// The extension follows the container the bytes were encoded in, so cache files stay
    /// readable by an operator (and by `file`).
    fn thumbnail_file_path(&self, repo_id: &str, path: &str, size: u32, ext: &str) -> PathBuf {
        let hash = thumbnail_key(repo_id, path);
        self.thumbnail_repo_dir(repo_id)
            .join(format!("{hash}_{size}.{ext}"))
    }

    /// Get or generate a thumbnail for a file.
    ///
    /// Returns the bytes, a strong ETag (SHA-1 of them) for `If-None-Match`, and the container so
    /// the handler can set `Content-Type`.
    pub async fn get_thumbnail(
        &self,
        repo_id: &str,
        path: &str,
        size: u32,
    ) -> Result<(Vec<u8>, String, ThumbFormat), AppError> {
        let normalized_path = if path.is_empty() || path == "/" {
            "/".to_string()
        } else if path.starts_with('/') {
            path.to_string()
        } else {
            format!("/{path}")
        };

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

        // Resolve fs_id and mtime in one tree walk; the mtime comes from the final hop's dirent.
        let (file_fs_id, current_mtime) =
            resolve_file_entry(&self.repos, repo_id, &head_commit.root_id, &normalized_path)
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

        // Parse the block list once so neither thumbnail source re-resolves the path.
        let file_data: FsFileData = serde_json::from_str(&file_obj.data)
            .map_err(|e| AppError::Internal(format!("invalid file object: {e}")))?;

        let existing = self
            .repos
            .thumbnail
            .find_by_repo_path_size(repo_id, &normalized_path, size as i32)
            .await?;

        if let Some(record) = existing {
            // The container is part of the cache entry: a hit must not decode to find its type.
            let format = ThumbFormat::from_db(&record.format);
            let thumbnail_path =
                self.thumbnail_file_path(repo_id, &normalized_path, size, format.ext());
            if record.file_modified_at >= current_mtime && thumbnail_path.exists() {
                let data = tokio::fs::read(&thumbnail_path)
                    .await
                    .map_err(|e| AppError::Internal(e.to_string()))?;
                let etag = etag_for(&data);
                return Ok((data, etag, format));
            }
        }

        // Everything below is expensive (a decode or a child process). Queue on the global gate
        // after the cache check so cached hits stay free, and hold the permit until the write.
        let _permit = acquire_thumbnail_permit().await?;

        // Images (in-process, or ffmpeg for HEIC/HEIF/AVIF) and audio/video (a frame or cover
        // via ffmpeg) have thumbnails; anything else does not.
        let ext = std::path::Path::new(&file_name)
            .extension()
            .and_then(|e| e.to_str())
            .map(|e| e.to_lowercase())
            .unwrap_or_default();
        let is_image = crate::thumbnail_util::is_supported_image_ext(&ext);
        let is_ffmpeg_image = crate::thumbnail_util::is_ffmpeg_image_ext(&ext);
        let is_video = crate::thumbnail_util::is_video_ext(&ext);
        let is_audio = crate::thumbnail_util::is_audio_ext(&ext);
        if !is_image && !is_ffmpeg_image && !is_video && !is_audio {
            return Err(AppError::NotFound("thumbnail not available".into()));
        }

        let (thumbnail_data, format) = if is_image {
            // Cap the in-memory read: decoding a multi-hundred-MB image wastes CPU and memory.
            const MAX_THUMBNAIL_SOURCE: i64 = 32 * 1024 * 1024;
            if file_data.size > MAX_THUMBNAIL_SOURCE {
                return Err(AppError::NotFound("thumbnail not available".into()));
            }

            let content = Downloader::read_file_limited_from_blocks(
                repo_id,
                &self.block_store,
                &file_data.block_ids,
                file_data.size,
                None,
                MAX_THUMBNAIL_SOURCE as usize,
            )
            .await
            .map_err(|_| AppError::NotFound("thumbnail not available".into()))?;

            // Decode, resize and encode all happen in the confined child: images are hostile input.
            tokio::task::spawn_blocking(move || images::thumbnail(&content, size))
                .await
                .map_err(|e| AppError::Internal(format!("thumbnail worker panicked: {e}")))?
                .map_err(|_| AppError::NotFound("thumbnail not available".into()))?
        } else {
            let kind = if is_video {
                MediaKind::Video
            } else if is_ffmpeg_image {
                MediaKind::Image
            } else {
                MediaKind::Audio
            };
            self.generate_media_thumbnail(repo_id, &normalized_path, size, kind, &file_data)
                .await?
        };

        let thumbnail_dir = self.thumbnail_repo_dir(repo_id);
        tokio::fs::create_dir_all(&thumbnail_dir).await?;
        let thumbnail_path =
            self.thumbnail_file_path(repo_id, &normalized_path, size, format.ext());
        let _ = tokio::fs::write(&thumbnail_path, &thumbnail_data).await;

        // Drop a stale entry in the other container, which would otherwise stay as an unreadable
        // orphan until `cleanup()` runs on delete.
        for other in [ThumbFormat::Png, ThumbFormat::Jpeg] {
            if other != format {
                let stale = self.thumbnail_file_path(repo_id, &normalized_path, size, other.ext());
                let _ = tokio::fs::remove_file(&stale).await;
            }
        }

        let now = chrono::Utc::now().timestamp();
        if let Some(_record) = self
            .repos
            .thumbnail
            .find_by_repo_path_size(repo_id, &normalized_path, size as i32)
            .await?
        {
            self.repos
                .thumbnail
                .update_mtime(
                    repo_id,
                    &normalized_path,
                    size as i32,
                    format.as_db(),
                    current_mtime,
                    now,
                )
                .await?;
            // Remove an old-scheme disk file (migration cleanup).
            let legacy_path = self
                .thumbnail_dir
                .join(thumbnail_dir_name(repo_id))
                .join(format!(
                    "{}_{}.png",
                    normalize_path_for_file(&normalized_path),
                    size
                ));
            let _ = tokio::fs::remove_file(&legacy_path).await;
        } else {
            self.repos
                .thumbnail
                .create(
                    repo_id,
                    &normalized_path,
                    size as i32,
                    format.as_db(),
                    current_mtime,
                    now,
                )
                .await?;
        }

        let etag = etag_for(&thumbnail_data);
        Ok((thumbnail_data, etag, format))
    }

    /// Generate a thumbnail for an audio/video file in the confined worker.
    ///
    /// The head of the file is streamed to a scratch file under `temp_dir` because the helper must
    /// seek and a pipe cannot. For video a frame is captured ~1s in (falling back to the first
    /// frame); for audio the embedded cover art is extracted. The helper, the decode of what it
    /// produced and the resize all run in the sandbox worker; nothing of the media reaches this
    /// process. Returns `NotFound` when the helper is unavailable or no frame/cover exists, so the
    /// UI falls back to an extension badge / play icon.
    ///
    /// Only the *head* is streamed: a first frame, cover art and a faststart index all sit at
    /// offset zero (see [`media_head_cap`]), which keeps a multi-gigabyte upload off disk. When the
    /// head misses a container whose index is at the end and the file fits the per-kind cap, the
    /// whole file is fetched once as a fallback. Images (HEIC/AVIF) are not streams, so they keep
    /// the whole-file cap unchanged.
    async fn generate_media_thumbnail(
        &self,
        repo_id: &str,
        normalized_path: &str,
        size: u32,
        kind: MediaKind,
        file_data: &FsFileData,
    ) -> Result<(Vec<u8>, ThumbFormat), AppError> {
        if !ffmpeg_available(&self.ffmpeg_path) {
            return Err(AppError::NotFound("thumbnail not available".into()));
        }

        // Images cannot decode from a truncated read, so they need the whole file; a >256 MiB
        // image is refused, since it could never be decoded from a prefix anyway.
        if kind == MediaKind::Image && file_data.size > max_ffmpeg_source(kind) {
            return Err(AppError::NotFound("thumbnail not available".into()));
        }

        let ffmpeg = crate::sandbox::worker::resolve_helper(self.ffmpeg_path.as_str());
        let file_size = file_data.size as u64;

        // Try the head first: a prefix is enough for the common faststart case.
        let head_cap = media_head_cap(kind);
        if let Some(source) = self
            .write_media_scratch(repo_id, normalized_path, &file_data.block_ids, head_cap)
            .await?
        {
            match Self::run_media_thumbnail(kind, &source, size, &ffmpeg).await {
                Ok(ok) => {
                    let _ = tokio::fs::remove_file(&source).await;
                    return Ok(ok);
                }
                // A panic is a worker failure, not an unthumbnailable file: propagate it.
                Err(AppError::Internal(e)) => {
                    let _ = tokio::fs::remove_file(&source).await;
                    return Err(AppError::Internal(e));
                }
                Err(_) => {
                    let _ = tokio::fs::remove_file(&source).await;
                }
            }
        }

        // Fallback: reading the rest is only worth it within the cap. A file past it failed above
        // for images and is left to fail for huge video/audio, where the full read is the cost.
        let fallback = if kind != MediaKind::Image
            && file_size > head_cap
            && file_data.size <= max_ffmpeg_source(kind)
        {
            self.write_media_scratch(
                repo_id,
                normalized_path,
                &file_data.block_ids,
                max_ffmpeg_source(kind) as u64,
            )
            .await?
        } else {
            None
        };
        if let Some(source) = fallback {
            match Self::run_media_thumbnail(kind, &source, size, &ffmpeg).await {
                Ok(ok) => {
                    let _ = tokio::fs::remove_file(&source).await;
                    return Ok(ok);
                }
                Err(AppError::Internal(e)) => {
                    let _ = tokio::fs::remove_file(&source).await;
                    return Err(AppError::Internal(e));
                }
                Err(_) => {
                    let _ = tokio::fs::remove_file(&source).await;
                }
            }
        }

        Err(AppError::NotFound("thumbnail not available".into()))
    }

    /// Stream at most `cap` bytes of a file into a fresh, unique scratch file.
    ///
    /// Returns `Ok(None)` on a write or stream error (nothing usable was written), which the
    /// caller treats as an unthumbnailable file.
    async fn write_media_scratch(
        &self,
        repo_id: &str,
        normalized_path: &str,
        block_ids: &[String],
        cap: u64,
    ) -> Result<Option<PathBuf>, AppError> {
        let scratch_dir = self.temp_dir.join("media_thumbs");
        tokio::fs::create_dir_all(&scratch_dir)
            .await
            .map_err(|e| AppError::Internal(format!("create scratch dir failed: {e}")))?;
        // Unique per request, so concurrent misses for one path cannot truncate each other's stream.
        let scratch = scratch_dir.join(format!(
            "{}_{}_{}.bin",
            thumbnail_dir_name(repo_id),
            thumbnail_key(repo_id, normalized_path),
            uuid::Uuid::new_v4()
        ));

        let written: Result<(), std::io::Error> = async {
            let mut out = tokio::fs::File::create(&scratch).await?;
            let mut stream = crate::fs::core::download::stream_blocks(
                repo_id.to_string(),
                block_ids.to_vec(),
                self.block_store.clone(),
                None,
            );
            let mut total: u64 = 0;
            while let Some(chunk) = stream.next().await {
                let chunk = chunk?;
                let remaining = cap.saturating_sub(total);
                if remaining == 0 {
                    break;
                }
                let take = remaining.min(chunk.len() as u64) as usize;
                out.write_all(&chunk[..take]).await?;
                total += take as u64;
                if take < chunk.len() {
                    break;
                }
            }
            out.flush().await
        }
        .await;

        match written {
            Ok(()) => Ok(Some(scratch)),
            Err(_) => {
                let _ = tokio::fs::remove_file(&scratch).await;
                Ok(None)
            }
        }
    }

    /// Hand a scratch file to the confined media worker and map its answer onto the service's
    /// result type. The caller removes the scratch on either outcome, including a worker panic.
    async fn run_media_thumbnail(
        kind: MediaKind,
        source: &Path,
        size: u32,
        ffmpeg: &Path,
    ) -> Result<(Vec<u8>, ThumbFormat), AppError> {
        let source = source.to_path_buf();
        let ffmpeg = ffmpeg.to_path_buf();
        let extracted =
            tokio::task::spawn_blocking(move || media::thumbnail(kind, &source, size, &ffmpeg))
                .await;
        extracted
            .map_err(|e| AppError::Internal(format!("media thumbnail worker panicked: {e}")))?
            .map_err(|_| AppError::NotFound("thumbnail not available".into()))
    }

    /// Remove all cached thumbnails (disk + DB) for a repo path; called when a file is deleted.
    pub async fn cleanup(&self, repo_id: &str, path: &str) {
        let normalized = if path.is_empty() || path == "/" {
            "/"
        } else if path.starts_with('/') {
            path
        } else {
            return; // non-absolute paths shouldn't happen
        };

        let _ = self
            .repos
            .thumbnail
            .delete_by_path(repo_id, normalized)
            .await;

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
}

/// Marker file recording that the legacy-size purge has run for the current retained sizes.
///
/// Its presence makes the purge one-shot; deleting it re-arms it. The name carries the size set
/// (`.legacy_sizes_purged_48_640`) on purpose: when a UI change drops a size, a new marker name
/// re-runs the purge by itself instead of leaving those files as permanent dead weight. (The
/// database rows need a new migration for the same reason — a migration is frozen once shipped —
/// which is `m20260918_000002_purge_legacy_thumbnail_sizes`.)
pub fn legacy_purge_marker() -> String {
    let sizes: Vec<String> = crate::thumbnail_util::RETAINED_THUMBNAIL_SIZES
        .iter()
        .map(|size| size.to_string())
        .collect();
    format!(".legacy_sizes_purged_{}", sizes.join("_"))
}

/// What one pass of the purge did, for the startup log.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct ThumbnailCachePurge {
    /// False when the marker showed a previous pass had already run.
    pub ran: bool,
    pub files_removed: u64,
    pub bytes_freed: u64,
    pub dirs_removed: u64,
    /// Files whose name carries no trailing size: nothing can reference them, so leave them for an
    /// operator rather than guess at their meaning.
    pub unrecognized: u64,
}

impl ThumbnailCachePurge {
    pub fn summary(&self) -> String {
        if !self.ran {
            return "already purged, skipped".to_string();
        }
        format!(
            "removed {} cache files ({} KiB) for sizes the UI no longer requests, \
             {} empty directories{}",
            self.files_removed,
            self.bytes_freed / 1024,
            self.dirs_removed,
            if self.unrecognized > 0 {
                format!(", {} unrecognized names left alone", self.unrecognized)
            } else {
                String::new()
            }
        )
    }
}

/// Remove the media-thumbnail scratch files an earlier run left behind.
///
/// Every request writes one under `{temp_dir}/media_thumbs/` and removes it when done, but a tray
/// quit ends the process via `std::process::exit`, which runs no destructor, and a terminated child
/// may still hold the file open on Windows, where the delete then fails. Nothing else sweeps the
/// directory, so it is swept once at startup, like `{temp_dir}/upload/`. Only top-level files are
/// removed and a failure is ignored: a leftover costs disk, and the request about to write one does
/// not need this to succeed first.
pub async fn purge_media_scratch(temp_dir: &Path) -> usize {
    let directory = temp_dir.join("media_thumbs");
    let Ok(mut entries) = tokio::fs::read_dir(&directory).await else {
        return 0;
    };
    let mut removed = 0;
    // A `read_dir` error mid-walk (a removed directory, a changed permission) ends the sweep.
    while let Ok(Some(entry)) = entries.next_entry().await {
        if tokio::fs::remove_file(entry.path()).await.is_ok() {
            removed += 1;
        }
    }
    removed
}

/// Delete thumbnail cache files for sizes the UI no longer requests.
///
/// The database half lives in the `purge_legacy_thumbnail_sizes` migration; the file half cannot,
/// because a migration has no access to the configured cache directory.
///
/// A marker named after the retained sizes (see [`legacy_purge_marker`]) makes it one-shot and is
/// written only after a pass completes, so a restart does not walk the cache again and an
/// interrupted pass finishes on the next start (deleting a deleted file is a no-op). The marker
/// lives in the cache directory itself, so a different cache directory re-runs the purge there.
///
/// Only this layout is assumed: one directory per repo (the name is a hash, so it is not decoded),
/// each holding `…_<size>.<ext>` entries. Deeper levels are not descended into, and a file that does
/// not end in `_<size>` is counted and skipped.
pub async fn purge_legacy_cache_files(
    thumbnail_dir: &Path,
) -> Result<ThumbnailCachePurge, AppError> {
    let marker = thumbnail_dir.join(legacy_purge_marker());
    if tokio::fs::try_exists(&marker).await? {
        return Ok(ThumbnailCachePurge::default());
    }

    let mut report = ThumbnailCachePurge {
        ran: true,
        ..Default::default()
    };

    let mut repos = tokio::fs::read_dir(thumbnail_dir).await?;
    while let Some(repo_dir) = repos.next_entry().await? {
        if !repo_dir.file_type().await?.is_dir() {
            continue;
        }
        let dir = repo_dir.path();
        let mut kept = 0u64;
        let mut files = tokio::fs::read_dir(&dir).await?;
        while let Some(file) = files.next_entry().await? {
            if !file.file_type().await?.is_file() {
                // A nested directory keeps its parent alive.
                kept += 1;
                continue;
            }
            let name = file.file_name();
            let Some(size) = cache_file_size(&name.to_string_lossy()) else {
                report.unrecognized += 1;
                kept += 1;
                continue;
            };
            if crate::thumbnail_util::RETAINED_THUMBNAIL_SIZES.contains(&size) {
                kept += 1;
                continue;
            }
            let bytes = file.metadata().await.map(|m| m.len()).unwrap_or(0);
            match tokio::fs::remove_file(file.path()).await {
                Ok(()) => {
                    report.files_removed += 1;
                    report.bytes_freed += bytes;
                }
                // Report it and keep going: one locked file must not stop the sweep, and it counts
                // as kept so its directory stays.
                Err(e) => {
                    tracing::warn!(
                        file = %file.path().display(),
                        "could not remove a stale thumbnail: {e}"
                    );
                    kept += 1;
                }
            }
        }
        if kept == 0 && tokio::fs::remove_dir(&dir).await.is_ok() {
            report.dirs_removed += 1;
        }
    }

    tokio::fs::write(&marker, b"done\n").await?;
    Ok(report)
}

/// Size encoded in a cache file name: `…_<size>.<ext>`, covering both the hashed scheme
/// (`thumb_<hash>_640.jpg`) and the pre-hash one (`Field_Photos_pine_256.png`).
fn cache_file_size(name: &str) -> Option<u32> {
    let stem = name.rsplit_once('.').map(|(stem, _)| stem).unwrap_or(name);
    stem.rsplit_once('_')
        .and_then(|(_, size)| size.parse::<u32>().ok())
}

/// Source-file size cap for ffmpeg thumbnails. Still images (HEIC/AVIF) are capped tighter than
/// video/audio: decoding a huge image needs much memory for little benefit.
fn max_ffmpeg_source(kind: MediaKind) -> i64 {
    match kind {
        MediaKind::Image => 256 * 1024 * 1024,
        MediaKind::Video | MediaKind::Audio => 512 * 1024 * 1024,
    }
}

/// How much of a video/audio file is streamed to the scratch file for a frame: a frame and any
/// embedded cover art sit at the start, so this prefix covers the common (faststart) case. A
/// container whose index is at the end falls back to the full file only when it fits
/// [`max_ffmpeg_source`]. `Image` (HEIC/AVIF) is not a stream and keeps the whole-file cap.
const MEDIA_HEAD_BYTES: u64 = 32 * 1024 * 1024;

fn media_head_cap(kind: MediaKind) -> u64 {
    match kind {
        MediaKind::Image => max_ffmpeg_source(kind) as u64,
        MediaKind::Video | MediaKind::Audio => MEDIA_HEAD_BYTES,
    }
}

/// Whether the configured ffmpeg binary looks runnable.
///
/// A filesystem check, not `ffmpeg -version` in the server process: that would be an unconfined exec
/// on the request path, and everything a helper reads belongs behind the sandbox. Whether the
/// binary actually *works* is what the confined media probe answers (`parse=media-ok`), which the
/// request path gates on through `sandbox::available`.
///
/// Not cached: saving `storage.ffmpeg_path` re-points the setting, so a process-lifetime answer
/// would keep reporting the previous path.
fn ffmpeg_available(ffmpeg: &str) -> bool {
    let path = crate::sandbox::worker::resolve_helper(ffmpeg);
    if !path.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::metadata(&path)
            .map(|metadata| metadata.permissions().mode() & 0o111 != 0)
            .unwrap_or(false)
    }
    #[cfg(not(unix))]
    {
        true
    }
}

/// Strong ETag for a thumbnail: SHA-1 of the bytes served, so the validator always reflects exactly
/// what went out (even when a stale cached entry was served by mistake).
fn etag_for(data: &[u8]) -> String {
    format!("\"{}\"", infra::crypto::fs_id::sha1_hex(data))
}

/// Filesystem-safe directory name for a repo's thumbnail cache.
///
/// `repo_id` is client-supplied, so it must never be a path segment: a value such as `../../x` or
/// `/etc/cron.d/y` would resolve outside `thumbnail_dir`. Hashing keeps the mapping stable and
/// collision-free while removing all path semantics.
fn thumbnail_dir_name(repo_id: &str) -> String {
    infra::crypto::fs_id::sha1_hex(repo_id.as_bytes())
}

/// Deterministic, collision-free filename prefix for a thumbnail: SHA-256(repo_id + path), matching
/// seahub's `generate_thumbnail_key()` but with SHA-256 (already a dependency) instead of MD5.
fn thumbnail_key(repo_id: &str, path: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(repo_id.as_bytes());
    hasher.update(path.as_bytes());
    let hash = hex::encode(hasher.finalize());
    // Use first 32 hex chars (128 bits) — plenty for collision avoidance
    format!("thumb_{}", &hash[..32])
}

/// Old path-normalization helper kept only for migration cleanup; `thumbnail_key()` replaced it.
fn normalize_path_for_file(path: &str) -> String {
    path.trim_matches('/').replace('/', "_")
}

#[cfg(test)]
mod tests {
    use super::*;

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

        let ok = media::grab_frame(Path::new("ffmpeg"), MediaKind::Video, &src);
        let _ = std::fs::remove_file(&src);
        let bytes = ok.expect("ffmpeg frame extraction failed");

        let decoded =
            image::load_from_memory(&bytes).expect("extracted frame is not a valid image");
        assert!(decoded.width() > 0 && decoded.height() > 0);

        let thumb = crate::thumbnail_util::generate_thumbnail(&bytes, 48)
            .expect("thumbnail fitter failed on extracted frame");
        assert!(!thumb.is_empty());
    }

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

        let ok = media::grab_frame(Path::new("ffmpeg"), MediaKind::Audio, &src);
        let _ = std::fs::remove_file(&src);
        let bytes = ok.expect("audio cover extraction failed");

        let decoded =
            image::load_from_memory(&bytes).expect("extracted cover is not a valid image");
        assert!(decoded.width() > 0 && decoded.height() > 0);

        let thumb = crate::thumbnail_util::generate_thumbnail(&bytes, 48)
            .expect("thumbnail fitter failed on extracted cover");
        assert!(!thumb.is_empty());
    }

    #[test]
    fn extract_image_writes_png() {
        if !ffmpeg_available("ffmpeg") {
            eprintln!("ffmpeg not available; skipping image thumbnail test");
            return;
        }

        let dir = std::env::temp_dir().join(format!(
            "nanofile_ithumb_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let src = dir.join("test.jpg");

        let status = Command::new("ffmpeg")
            .arg("-y")
            .arg("-loglevel")
            .arg("error")
            .arg("-f")
            .arg("lavfi")
            .arg("-i")
            .arg("testsrc=size=320x240:rate=1:duration=1")
            .arg("-frames:v")
            .arg("1")
            .arg(&src)
            .status()
            .expect("failed to spawn ffmpeg");
        assert!(status.success(), "ffmpeg failed to create test image");

        let ok = media::grab_frame(Path::new("ffmpeg"), MediaKind::Image, &src);
        let _ = std::fs::remove_file(&src);
        let bytes = ok.expect("ffmpeg image extraction failed");

        let decoded =
            image::load_from_memory(&bytes).expect("extracted image is not a valid image");
        assert!(decoded.width() > 0 && decoded.height() > 0);

        let thumb = crate::thumbnail_util::generate_thumbnail(&bytes, 48)
            .expect("thumbnail fitter failed on extracted image");
        assert!(!thumb.is_empty());
    }

    #[test]
    fn tiff_in_process_thumbnail() {
        let mut img = image::RgbaImage::new(64, 64);
        for p in img.pixels_mut() {
            *p = image::Rgba([10u8, 20, 30, 255]);
        }
        let mut bytes = std::io::Cursor::new(Vec::new());
        img.write_to(&mut bytes, image::ImageFormat::Tiff)
            .expect("TIFF encode failed");
        let data = bytes.into_inner();

        let thumb = crate::thumbnail_util::generate_thumbnail(&data, 32)
            .expect("thumbnail fitter failed on TIFF");
        assert!(!thumb.is_empty());
    }

    /// The extension classification is the single source of truth for which format takes which
    /// thumbnail path.
    #[test]
    fn extension_classification() {
        use crate::thumbnail_util::{
            is_ffmpeg_image_ext, is_supported_image_ext, is_thumbnail_image_ext,
        };

        assert!(is_supported_image_ext("tiff"));
        assert!(is_supported_image_ext("tif"));
        assert!(!is_supported_image_ext("heic"));
        assert!(!is_supported_image_ext("avif"));

        assert!(is_ffmpeg_image_ext("heic"));
        assert!(is_ffmpeg_image_ext("heif"));
        assert!(is_ffmpeg_image_ext("avif"));
        assert!(!is_ffmpeg_image_ext("png"));
        assert!(!is_ffmpeg_image_ext("tiff"));

        assert!(is_thumbnail_image_ext("tiff"));
        assert!(is_thumbnail_image_ext("heic"));
        assert!(is_thumbnail_image_ext("avif"));
        assert!(!is_thumbnail_image_ext("svg"));
    }
}

#[cfg(test)]
mod concurrency_tests {
    use super::acquire_thumbnail_permit;
    use crate::sandbox::jobs::images::MAX_CONCURRENT_IMAGE_WORKERS;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// The gate is process-global (`thumbnail_service()` builds a fresh service per request), so
    /// permits acquired from unrelated call sites must still share one cap.
    #[tokio::test]
    async fn concurrent_generations_are_capped() {
        let in_flight = Arc::new(AtomicUsize::new(0));
        let peak = Arc::new(AtomicUsize::new(0));

        let mut tasks = Vec::new();
        for _ in 0..(MAX_CONCURRENT_IMAGE_WORKERS * 3) {
            let in_flight = in_flight.clone();
            let peak = peak.clone();
            tasks.push(tokio::spawn(async move {
                let _permit = acquire_thumbnail_permit().await.unwrap();
                let now = in_flight.fetch_add(1, Ordering::SeqCst) + 1;
                peak.fetch_max(now, Ordering::SeqCst);
                tokio::time::sleep(std::time::Duration::from_millis(20)).await;
                in_flight.fetch_sub(1, Ordering::SeqCst);
            }));
        }
        for task in tasks {
            task.await.unwrap();
        }

        assert_eq!(
            peak.load(Ordering::SeqCst),
            MAX_CONCURRENT_IMAGE_WORKERS,
            "the gate should admit exactly the configured number of generators"
        );
    }
}

#[cfg(test)]
mod purge_tests {
    use super::{
        cache_file_size, legacy_purge_marker, purge_legacy_cache_files, purge_media_scratch,
    };
    use std::path::Path;

    /// Both naming schemes end in `_<size>.<ext>`; a name carrying no trailing size is reported so
    /// the purge can skip it.
    #[test]
    fn cache_file_size_reads_both_naming_schemes() {
        assert_eq!(
            cache_file_size("thumb_e49c1f0fdb7fdd0c0627136cf973f6ff_48.png"),
            Some(48)
        );
        assert_eq!(cache_file_size("thumb_ab_640.jpg"), Some(640));
        assert_eq!(cache_file_size("Field_Photos_pine_256.png"), Some(256));
        assert_eq!(cache_file_size("thumb_ab_48"), Some(48));
        assert_eq!(cache_file_size("notes.txt"), None);
        assert_eq!(cache_file_size("thumb_ab_.png"), None);
        assert_eq!(cache_file_size(&legacy_purge_marker()), None);
    }

    /// The marker names the retained size set, so changing that set re-arms the purge without a
    /// version bump.
    #[test]
    fn marker_names_the_retained_sizes() {
        assert_eq!(legacy_purge_marker(), ".legacy_sizes_purged_48_640");
    }

    fn write(dir: &Path, name: &str, bytes: usize) {
        std::fs::create_dir_all(dir).unwrap();
        std::fs::write(dir.join(name), vec![0u8; bytes]).unwrap();
    }

    fn names(dir: &Path) -> Vec<String> {
        let mut v: Vec<String> = std::fs::read_dir(dir)
            .into_iter()
            .flatten()
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        v.sort();
        v
    }

    #[tokio::test]
    async fn purge_removes_only_unreachable_sizes_and_runs_once() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let repo_a = root.join("repo-a");
        let repo_b = root.join("repo-b");

        write(&repo_a, "thumb_aaa_48.png", 10);
        write(&repo_a, "thumb_aaa_640.jpg", 20);
        write(&repo_a, "thumb_aaa_256.png", 100);
        write(&repo_a, "thumb_bbb_512.png", 200);
        write(&repo_a, "notes.txt", 5);
        // A nested level is never descended into, so its contents survive.
        write(&repo_a.join("nested"), "thumb_ccc_256.png", 50);
        write(&repo_b, "thumb_ddd_256.png", 7);

        let report = purge_legacy_cache_files(root).await.unwrap();
        assert!(report.ran);
        assert_eq!(report.files_removed, 3, "256 and 512 entries only");
        assert_eq!(report.bytes_freed, 100 + 200 + 7);
        assert_eq!(report.dirs_removed, 1, "repo-b held nothing else");
        assert_eq!(report.unrecognized, 1, "notes.txt has no size to read");
        assert!(report.summary().contains("removed 3 cache files"));

        assert_eq!(
            names(&repo_a),
            [
                "nested",
                "notes.txt",
                "thumb_aaa_48.png",
                "thumb_aaa_640.jpg"
            ]
        );
        assert_eq!(
            names(&repo_a.join("nested")),
            ["thumb_ccc_256.png"],
            "the purge must not walk deeper than one level"
        );
        assert!(
            !repo_b.exists(),
            "a repo directory emptied by the purge is removed"
        );
        assert!(root.join(legacy_purge_marker()).exists());

        // The marker short-circuits even with a new stale file present.
        write(&repo_a, "thumb_eee_256.png", 30);
        let again = purge_legacy_cache_files(root).await.unwrap();
        assert!(!again.ran, "the marker makes this a one-shot");
        assert_eq!(again, super::ThumbnailCachePurge::default());
        assert!(repo_a.join("thumb_eee_256.png").exists());
    }

    /// A stale file that cannot be removed keeps its repo directory alive and is counted as kept,
    /// but does not abort the sweep.
    #[tokio::test]
    async fn an_unremovable_entry_does_not_abort_the_sweep() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let repo = root.join("repo-a");
        // `thumb_x_256.png` as a *directory* — `remove_file` fails on it.
        std::fs::create_dir_all(repo.join("thumb_x_256.png")).unwrap();
        write(&repo, "thumb_y_256.png", 12);

        let report = purge_legacy_cache_files(root).await.unwrap();
        assert_eq!(
            report.files_removed, 1,
            "the file beside it was still removed"
        );
        assert_eq!(
            report.dirs_removed, 0,
            "the directory entry keeps the repo dir"
        );
        assert!(repo.join("thumb_x_256.png").is_dir());
    }

    #[tokio::test]
    async fn the_media_scratch_of_a_previous_run_is_swept_and_the_rest_left_alone() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let scratch = root.join("media_thumbs");
        std::fs::create_dir_all(&scratch).unwrap();
        std::fs::write(scratch.join("repo-a_key_1.bin"), b"leftover").unwrap();
        std::fs::write(scratch.join("repo-b_key_2.bin"), b"leftover").unwrap();
        // A directory is not a scratch file, and the sibling of the scratch directory is not this
        // sweep's business.
        std::fs::create_dir_all(scratch.join("not-a-file")).unwrap();
        std::fs::create_dir_all(root.join("upload")).unwrap();
        std::fs::write(root.join("upload").join("staged"), b"staged").unwrap();

        assert_eq!(purge_media_scratch(root).await, 2);
        assert!(!scratch.join("repo-a_key_1.bin").exists());
        assert!(!scratch.join("repo-b_key_2.bin").exists());
        assert!(scratch.join("not-a-file").is_dir());
        assert!(root.join("upload").join("staged").exists());

        // A missing scratch directory is not an error, and neither is a second sweep.
        assert_eq!(purge_media_scratch(root).await, 0);
        assert_eq!(purge_media_scratch(&root.join("absent")).await, 0);
    }
}
