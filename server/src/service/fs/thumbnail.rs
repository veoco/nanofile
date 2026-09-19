use futures::StreamExt;
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::sync::OnceLock;
use tokio::io::AsyncWriteExt;

use crate::fs::core::download::Downloader;
use crate::fs::core::tree::resolve_file_entry;
use crate::repository::Repositories;
use crate::thumbnail_util::ThumbFormat;
use base::common::{EMPTY_SHA1, FsFileData, SEAF_METADATA_TYPE_DIR};
use base::error::AppError;

/// Cap on how many thumbnails are generated concurrently across the server.
///
/// A cache miss either decodes a large image in-process (up to 32 MiB of source)
/// or spawns an `ffmpeg` child that may read a 512 MiB media file, so a burst of
/// requests for distinct files would otherwise start unbounded subprocesses and
/// saturate CPU, memory and file descriptors. Matches the zip archive cap: a
/// hardcoded constant rather than a config knob, so it cannot be set to
/// "unlimited" by accident.
const MAX_CONCURRENT_THUMBNAILS: usize = 4;

static THUMBNAIL_CONCURRENCY: OnceLock<Arc<tokio::sync::Semaphore>> = OnceLock::new();

fn thumbnail_semaphore() -> &'static Arc<tokio::sync::Semaphore> {
    THUMBNAIL_CONCURRENCY
        .get_or_init(|| Arc::new(tokio::sync::Semaphore::new(MAX_CONCURRENT_THUMBNAILS)))
}

/// Acquire a permit covering one thumbnail generation. Callers queue rather
/// than fail, preserving the previous behaviour for a normal burst.
async fn acquire_thumbnail_permit() -> Result<tokio::sync::OwnedSemaphorePermit, AppError> {
    thumbnail_semaphore()
        .clone()
        .acquire_owned()
        .await
        .map_err(|e| AppError::Internal(format!("thumbnail concurrency gate failed: {e}")))
}

pub struct ThumbnailService {
    repos: Arc<Repositories>,
    block_store: infra::storage::DynBlockStorage,
    /// Thumbnail cache directory (configurable).
    thumbnail_dir: Arc<PathBuf>,
    /// Scratch directory for streaming video files before ffmpeg extracts a frame.
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
    /// The directory name is `sha1(repo_id)` rather than the raw id: the cache
    /// is keyed by a client-supplied identifier that must never be able to
    /// escape `thumbnail_dir` via `..` or an absolute path. The
    /// directory is a regenerable cache, so the naming scheme is internal.
    fn thumbnail_repo_dir(&self, repo_id: &str) -> PathBuf {
        self.thumbnail_dir.join(thumbnail_dir_name(repo_id))
    }

    /// Deterministic on-disk filename for a thumbnail, matching seahub's
    /// `generate_thumbnail_key()` approach but using MD5(repo_id + path)
    /// instead of a bare path (avoids path-collision bugs).
    ///
    /// The extension follows the container the bytes were encoded in, so the
    /// cache directory is readable by an operator (and by `file`).
    fn thumbnail_file_path(&self, repo_id: &str, path: &str, size: u32, ext: &str) -> PathBuf {
        let hash = thumbnail_key(repo_id, path);
        self.thumbnail_repo_dir(repo_id)
            .join(format!("{hash}_{size}.{ext}"))
    }

    /// Get or generate a thumbnail for a file.
    ///
    /// Returns the thumbnail data, a strong ETag (SHA-1 of the bytes) so the
    /// handler can serve `If-None-Match` conditional requests, and the
    /// container the bytes are in so it can set `Content-Type`.
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

        // Resolve the file's fs_id and current mtime in one tree walk (the
        // mtime comes from the final hop's parent dirent).
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

        // Parse the file object's block list once so both thumbnail sources
        // (image bytes / ffmpeg stream) avoid re-resolving the path.
        let file_data: FsFileData = serde_json::from_str(&file_obj.data)
            .map_err(|e| AppError::Internal(format!("invalid file object: {e}")))?;

        // ── Check if a valid cached thumbnail exists ──
        let existing = self
            .repos
            .thumbnail
            .find_by_repo_path_size(repo_id, &normalized_path, size as i32)
            .await?;

        if let Some(record) = existing {
            // The container is part of the cache entry: a hit must not decode
            // the bytes to work out its content type.
            let format = ThumbFormat::from_db(&record.format);
            let thumbnail_path =
                self.thumbnail_file_path(repo_id, &normalized_path, size, format.ext());
            // Staleness check: if source file was modified after the thumbnail was created, regenerate
            if record.file_modified_at >= current_mtime && thumbnail_path.exists() {
                let data = tokio::fs::read(&thumbnail_path)
                    .await
                    .map_err(|e| AppError::Internal(e.to_string()))?;
                let etag = etag_for(&data);
                return Ok((data, etag, format));
            }
            // Stale — fall through to regenerate
        }

        // Every path below is expensive (decode or a child process). Queue on
        // the global gate *after* the cache check so cached thumbnails stay
        // free, and hold the permit until the result has been written.
        let _permit = acquire_thumbnail_permit().await?;

        // Supported sources are images (decoded in-process, or via ffmpeg for
        // HEIC/HEIF/AVIF) and audio/video (a frame or embedded cover art
        // extracted via ffmpeg). Anything else has no thumbnail.
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
            // Skip huge images and cap the in-memory read; decoding a truncated
            // multi-hundred-MB image would waste CPU and memory for no benefit.
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

            tokio::task::spawn_blocking(move || {
                crate::thumbnail_util::generate_thumbnail_encoded(&content, size)
            })
            .await
            .map_err(|e| AppError::Internal(format!("thumbnail generation panicked: {e}")))?
            .map_err(|e| AppError::Internal(format!("thumbnail generation failed: {e}")))?
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

        // ── Store thumbnail for future requests ──
        let thumbnail_dir = self.thumbnail_repo_dir(repo_id);
        tokio::fs::create_dir_all(&thumbnail_dir).await?;
        let thumbnail_path =
            self.thumbnail_file_path(repo_id, &normalized_path, size, format.ext());
        let _ = tokio::fs::write(&thumbnail_path, &thumbnail_data).await;

        // Drop a stale entry in the other container: a size cached before the
        // encoding followed the pixels (or a file that changed from opaque to
        // transparent) would otherwise leave an unreadable orphan behind, and
        // `cleanup()` only runs on delete.
        for other in [ThumbFormat::Png, ThumbFormat::Jpeg] {
            if other != format {
                let stale = self.thumbnail_file_path(repo_id, &normalized_path, size, other.ext());
                let _ = tokio::fs::remove_file(&stale).await;
            }
        }

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
                .update_mtime(
                    repo_id,
                    &normalized_path,
                    size as i32,
                    format.as_db(),
                    current_mtime,
                    now,
                )
                .await?;
            // Delete old-naming disk file if it still exists (migration from old path scheme)
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

    /// Generate a thumbnail for an audio/video file via ffmpeg.
    ///
    /// The file is streamed to a scratch file under `temp_dir` so ffmpeg can
    /// seek. For video a frame is captured ~1s in (falling back to the first
    /// frame); for audio the embedded cover art is extracted. The extracted
    /// frame is fitted to `size` and encoded by the shared image util (JPEG —
    /// an extracted frame is opaque), so the caller gets the bytes and their
    /// container. Returns `NotFound` when ffmpeg is unavailable or no
    /// frame/cover exists — the UI then falls back to an extension badge / play
    /// icon.
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

        if file_data.size > max_ffmpeg_source(kind) {
            return Err(AppError::NotFound("thumbnail not available".into()));
        }

        // Stream the whole media file to a scratch file so ffmpeg can seek.
        let scratch_dir = self.temp_dir.join("media_thumbs");
        tokio::fs::create_dir_all(&scratch_dir)
            .await
            .map_err(|e| AppError::Internal(format!("create scratch dir failed: {e}")))?;
        // Unique per request: two concurrent misses for the same path used to
        // share this filename and truncate each other's source stream mid-write.
        let scratch_media = scratch_dir.join(format!(
            "{}_{}_{}.bin",
            thumbnail_dir_name(repo_id),
            thumbnail_key(repo_id, normalized_path),
            uuid::Uuid::new_v4()
        ));
        let scratch_png = scratch_media.with_extension("png");

        let write_result: Result<(), std::io::Error> = async {
            let mut out = tokio::fs::File::create(&scratch_media).await?;
            let mut stream = crate::fs::core::download::stream_blocks(
                repo_id.to_string(),
                file_data.block_ids.clone(),
                self.block_store.clone(),
                None,
            );
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

        tokio::task::spawn_blocking(move || {
            crate::thumbnail_util::generate_thumbnail_encoded(&png, size)
        })
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
}

// ─── One-shot legacy-size purge ───────────────────────────────────────────

/// Marker file recording that the legacy-size purge has run for the current set
/// of retained sizes. Its presence is what makes the purge one-shot rather than
/// a startup cost on every boot; deleting it re-arms the purge.
///
/// The name carries the size set (`.legacy_sizes_purged_48_640`) on purpose:
/// when a future UI changes the sizes it requests, every entry at the previous
/// size becomes unreachable, and a new marker name makes the purge run again by
/// itself instead of leaving those files behind as permanent dead weight. (The
/// database rows need a new migration for the same reason — a migration is
/// frozen once it has shipped — which is what
/// `m20260918_000002_purge_legacy_thumbnail_sizes` is.)
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
    /// Files whose name carries no trailing size. Left alone: nothing can
    /// reference them, but guessing at a cache file's meaning is worse than
    /// leaving the bytes for an operator to look at.
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

/// Delete thumbnail cache files for sizes the UI no longer requests.
///
/// The database half of this lives in the `purge_legacy_thumbnail_sizes`
/// migration; the file half cannot, because a schema migration has no access to
/// the configured cache directory. So the rows go during the upgrade and the
/// bytes go here, on the first start of the new binary.
///
/// Guarded by a marker named after the retained sizes (see
/// [`legacy_purge_marker`]), written only after a pass completes, so a server
/// that restarts does not walk the cache again — and a pass interrupted halfway
/// (crash, SIGKILL, disk full) simply finishes on the next start, since deleting
/// an already-deleted file is a no-op. The marker lives in the cache directory
/// itself, so pointing a server at a different cache directory re-runs the purge
/// there, which is what you want.
///
/// Only the layout of `thumbnail_dir` is assumed: one directory per repo (the
/// directory name is a hash, so it is not decoded), each holding
/// `…_<size>.<ext>` entries. Deeper levels are never descended into, and a file
/// that does not end in `_<size>` is counted and skipped.
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
                // Report it and keep going: one locked file must not stop the
                // sweep. It is still counted as kept, so its directory stays.
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

/// Size encoded in a cache file name: `…_<size>.<ext>`, which covers both the
/// hashed scheme (`thumb_<hash>_640.jpg`) and the pre-hash one
/// (`Field_Photos_pine_256.png`). `None` when the name carries no trailing size.
fn cache_file_size(name: &str) -> Option<u32> {
    let stem = name.rsplit_once('.').map(|(stem, _)| stem).unwrap_or(name);
    stem.rsplit_once('_')
        .and_then(|(_, size)| size.parse::<u32>().ok())
}

// ─── Helpers ──────────────────────────────────────────────────────────────

/// What kind of media a ffmpeg-based thumbnail should extract.
#[derive(Clone, Copy)]
enum MediaKind {
    /// A frame from the video stream (~1s in, first frame as fallback).
    Video,
    /// The embedded cover art (attached picture) of an audio file.
    Audio,
    /// A still image (HEIC/HEIF/AVIF) decoded by ffmpeg's image demuxer.
    Image,
}

/// Source-file size cap for ffmpeg thumbnails. Still images (HEIC/AVIF photos)
/// are capped tighter than video/audio — decoding a huge image needs lots of
/// memory for little benefit, and phone photos are typically a few MB.
fn max_ffmpeg_source(kind: MediaKind) -> i64 {
    match kind {
        MediaKind::Image => 256 * 1024 * 1024,
        MediaKind::Video | MediaKind::Audio => 512 * 1024 * 1024,
    }
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
/// - `Image`: decodes a still image (HEIC/HEIF/AVIF) via the image demuxer
///   (no seek, no `-map`).
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
        if run_with_timeout(&mut cmd, FFMPEG_TIMEOUT) {
            return true;
        }
    }
    false
}

/// Timeout for a single ffmpeg thumbnail-extraction attempt. A maliciously
/// crafted media file could otherwise make ffmpeg run indefinitely and tie up
/// CPU; kill the process once this deadline is reached.
const FFMPEG_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

/// Run a child process, waiting up to `timeout` for it to exit. Returns true on
/// a successful exit; kills the child (and returns false) on timeout or error.
fn run_with_timeout(cmd: &mut Command, timeout: std::time::Duration) -> bool {
    let Ok(mut child) = cmd.spawn() else {
        return false;
    };
    let deadline = std::time::Instant::now() + timeout;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return status.success(),
            Ok(None) => {
                if std::time::Instant::now() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    return false;
                }
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
            Err(_) => return false,
        }
    }
}

/// Strong ETag for a thumbnail: SHA-1 of the PNG bytes, so the validator
/// always reflects exactly what was served (even when the staleness check
/// mistakenly returns a cached image).
fn etag_for(data: &[u8]) -> String {
    format!("\"{}\"", infra::crypto::fs_id::sha1_hex(data))
}

/// Filesystem-safe directory name for a repo's thumbnail cache.
///
/// `repo_id` is a client-supplied identifier, so it must never be used as a
/// path segment directly: a value such as `../../x` or `/etc/cron.d/y` would
/// resolve outside `thumbnail_dir`. Hashing it keeps the mapping stable
/// and collision-free while removing all path semantics.
fn thumbnail_dir_name(repo_id: &str) -> String {
    infra::crypto::fs_id::sha1_hex(repo_id.as_bytes())
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

    /// Decode a still image (HEIC/HEIF/AVIF path) via `extract_media_frame` and
    /// verify the output is a decodable PNG the shared fitter accepts.
    /// Skips silently when ffmpeg isn't installed on the host.
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
        let dst = dir.join("frame.png");

        // Generate a single static test frame.
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

        let ok = extract_media_frame("ffmpeg", MediaKind::Image, &src, &dst);
        let _ = std::fs::remove_file(&src);
        assert!(ok, "ffmpeg image extraction failed");

        let bytes = std::fs::read(&dst).unwrap();
        let _ = std::fs::remove_file(&dst);
        let decoded =
            image::load_from_memory(&bytes).expect("extracted image is not a valid image");
        assert!(decoded.width() > 0 && decoded.height() > 0);

        let thumb = crate::thumbnail_util::generate_thumbnail(&bytes, 48)
            .expect("thumbnail fitter failed on extracted image");
        assert!(!thumb.is_empty());
    }

    /// Encode a synthetic TIFF in-process and verify the in-process thumbnail
    /// path (image crate + shared fitter) accepts it.
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

    /// The extension classification is the single source of truth for which
    /// format takes which thumbnail path.
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
    use super::{MAX_CONCURRENT_THUMBNAILS, acquire_thumbnail_permit};
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// The gate is process-global (`thumbnail_service()` builds a fresh service
    /// per request), so permits acquired from unrelated call sites must still
    /// share one cap.
    #[tokio::test]
    async fn concurrent_generations_are_capped() {
        let in_flight = Arc::new(AtomicUsize::new(0));
        let peak = Arc::new(AtomicUsize::new(0));

        let mut tasks = Vec::new();
        for _ in 0..(MAX_CONCURRENT_THUMBNAILS * 3) {
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
            MAX_CONCURRENT_THUMBNAILS,
            "the gate should admit exactly the configured number of generators"
        );
    }
}

#[cfg(test)]
mod purge_tests {
    use super::{cache_file_size, legacy_purge_marker, purge_legacy_cache_files};
    use std::path::Path;

    /// Both naming schemes (hashed and pre-hash) end in `_<size>.<ext>`; a name
    /// that carries no trailing size is reported so the purge can skip it.
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

    /// The marker names the retained size set, so changing that set re-arms the
    /// purge without anyone having to remember to bump a version.
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

    /// The purge removes exactly the unreachable sizes, keeps the retained
    /// ones, never recurses below the per-repo directory, leaves names it
    /// cannot parse alone, drops a directory it emptied, and does not run twice.
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

        // Second start: the marker short-circuits, even with a new stale file.
        write(&repo_a, "thumb_eee_256.png", 30);
        let again = purge_legacy_cache_files(root).await.unwrap();
        assert!(!again.ran, "the marker makes this a one-shot");
        assert_eq!(again, super::ThumbnailCachePurge::default());
        assert!(repo_a.join("thumb_eee_256.png").exists());
    }

    /// A stale file that cannot be removed (a directory in its place, for
    /// instance) keeps its repo directory alive and is counted as kept, but does
    /// not abort the sweep.
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
}
