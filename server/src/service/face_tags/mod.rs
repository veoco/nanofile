//! The face-tag scan pipeline: read an image, ask the confined face worker for a
//! face count, and materialise the `人物`/`单人`/`合影` tags.
//!
//! The only thing persisted about a face is its count and the best score. There
//! is no embedding, face crop or identity in this module.
//!
//! The detector has its own sandbox profile ([`crate::sandbox::Profile::Faces`]),
//! so a scan that fails or hangs cannot take thumbnails, avatars or EXIF down
//! with it. Where the image crate cannot decode the bytes — HEIC, HEIF, AVIF — a
//! still frame is decoded through the media profile first.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use futures::StreamExt;

use crate::face_tags::DetectOptions;
use crate::face_tags::MODEL_VERSION;
use crate::face_tags::labels::{self, LabelDef};
use crate::fs::core::download::Downloader;
use crate::fs::core::tree::read_fs_file_data;
use crate::repository::Repositories;
use crate::repository::face_tags::ScanRecord;
use crate::repository::repo_tag::TagInput;
use crate::sandbox::jobs::faces::{self, FaceError};
use crate::sandbox::jobs::media::{self, StillError};
use crate::tasks::context::JobContext;
use base::common::FsFileData;
use base::error::AppError;
use infra::storage::DynBlockStorage;

const MAX_IMAGE_BYTES: usize = 32 * 1024 * 1024;
const BACKFILL_BUDGET: Duration = Duration::from_secs(20);
const FAILED_RETRY_AFTER_SECS: i64 = 3600;
const CONCURRENCY: usize = 2;

/// Reason a HEIC/HEIF/AVIF scan was skipped for want of a decoder.
const SKIP_NO_FFMPEG: &str = "HEIC/AVIF face scan needs ffmpeg";

/// Skip reasons that are about this build's capability, not the bytes.
///
/// The first is what the release before HEIC/AVIF support wrote; the second is this
/// build's "no ffmpeg yet". A row with either is worth another attempt once the decode
/// path exists; any other reason is a verdict on the file.
const RETRY_WHEN_DECODABLE: [&str; 2] = ["HEIC/AVIF face scan not supported", SKIP_NO_FFMPEG];

/// What this file's bytes need before the detector can see them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ImageSource {
    /// The `image` crate decodes it: the bytes go straight to the face worker.
    InProcess,
    /// The `image` crate cannot, ffmpeg can: decode a still through the media profile
    /// first.
    Media,
    /// Neither: not an image this feature scans.
    NotAnImage,
}

fn source_for(ext: &str) -> ImageSource {
    if crate::thumbnail_util::is_supported_image_ext(ext) {
        ImageSource::InProcess
    } else if crate::thumbnail_util::is_ffmpeg_image_ext(ext) {
        ImageSource::Media
    } else {
        ImageSource::NotAnImage
    }
}

/// The lowercase extension of a path, or an empty string.
fn extension_of(path: &str) -> String {
    let filename = path.rsplit_once('/').map(|(_, name)| name).unwrap_or(path);
    std::path::Path::new(filename)
        .extension()
        .and_then(|ext| ext.to_str())
        .map(str::to_ascii_lowercase)
        .unwrap_or_default()
}

/// What one file's scan produced, before the tags are materialised.
enum ScanOutcome {
    Count(crate::face_tags::engine::DetectionSummary),
    /// This build will never scan these bytes: a skip, not retried.
    Skip(String),
    /// The environment or a timeout: a retryable failure.
    Retry(String),
}

/// A worker failure as the outcome it is.
///
/// A refused decode is a verdict on the bytes — the same file will not decode on the
/// next backfill pass either — so it is recorded as a skip. Everything else (a child
/// that could not start, a detector that failed) stays retryable.
fn outcome_for_failure(error: FaceError) -> ScanOutcome {
    match error {
        FaceError::Refused(reason) => {
            ScanOutcome::Skip(format!("image cannot be decoded: {reason}"))
        }
        FaceError::Retry(reason) => ScanOutcome::Retry(reason),
    }
}

/// Whether a stored scan row is worth another attempt for this content.
///
/// `wait_for_failed` is the caller's cadence: a scan job retries a failed row after the
/// hour, while the backfill pass is already rate-limited by its own idle window and
/// retries one it meets. A row skipped for want of a decoder is retried once one is
/// configured (`decodable`); any other skip is a verdict on the bytes.
fn needs_scan(
    previous: &infra::entity::face_tag_scan::Model,
    fs_id: &str,
    ext: &str,
    wait_for_failed: bool,
    decodable: bool,
) -> bool {
    if previous.fs_id != fs_id || previous.model_version != MODEL_VERSION as i32 {
        return true;
    }
    if previous.status == "failed" {
        return !wait_for_failed
            || chrono::Utc::now().timestamp() - previous.attempted_at >= FAILED_RETRY_AFTER_SECS;
    }
    source_for(ext) == ImageSource::Media
        && decodable
        && previous.status == "skipped"
        && previous
            .reason
            .as_deref()
            .is_some_and(|reason| RETRY_WHEN_DECODABLE.contains(&reason))
}

#[derive(Clone, Debug)]
pub struct ScanTarget {
    pub path: String,
    pub fs_id: String,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ScanCounts {
    pub indexed: u64,
    pub skipped: u64,
    pub failed: u64,
}

impl ScanCounts {
    pub fn total(&self) -> u64 {
        self.indexed + self.skipped + self.failed
    }

    pub fn add(&mut self, other: ScanCounts) {
        self.indexed += other.indexed;
        self.skipped += other.skipped;
        self.failed += other.failed;
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct BackfillReport {
    pub repos_scanned: u64,
    pub files_checked: u64,
    pub counts: ScanCounts,
}

pub struct FaceTagService {
    repos: Arc<Repositories>,
    block_store: DynBlockStorage,
    config: crate::settings::RuntimeConfig,
}

impl FaceTagService {
    pub fn new(
        repos: Arc<Repositories>,
        block_store: DynBlockStorage,
        config: crate::settings::RuntimeConfig,
    ) -> Self {
        Self {
            repos,
            block_store,
            config,
        }
    }

    /// Scan a batch of paths. One failed file does not fail the batch.
    pub async fn scan_paths(
        &self,
        ctx: Option<&JobContext>,
        repo_id: &str,
        targets: &[ScanTarget],
    ) -> Result<ScanCounts, AppError> {
        let total = targets.len() as u64;
        let mut done = 0u64;
        let mut counts = ScanCounts::default();
        let owned: Vec<ScanTarget> = targets.to_vec();
        let mut stream = futures::stream::iter(owned.into_iter().map(|target| {
            let ctx = ctx.cloned();
            async move {
                if let Some(ctx) = &ctx {
                    ctx.checkpoint()
                        .await
                        .map_err(|_| AppError::OperationFailed("face-tag scan cancelled".into()))?;
                }
                self.scan_one(ctx.as_ref(), repo_id, &target.path, &target.fs_id)
                    .await
            }
        }))
        .buffer_unordered(CONCURRENCY);

        while let Some(result) = stream.next().await {
            match result {
                Ok(one) => counts.add(one),
                Err(error) => {
                    tracing::warn!("face-tag scan failed for one file: {error}");
                    counts.failed += 1;
                }
            }
            done += 1;
            if let Some(ctx) = ctx {
                ctx.report(done, Some(total));
            }
        }
        Ok(counts)
    }

    async fn scan_one(
        &self,
        ctx: Option<&JobContext>,
        repo_id: &str,
        path: &str,
        fs_id: &str,
    ) -> Result<ScanCounts, AppError> {
        let config = self.config.get();
        if !config.face_tags.enabled {
            return Ok(ScanCounts::default());
        }

        // Most callers already know the content id; a manual batch may not.
        let resolved_fs_id;
        let fs_id = if fs_id.is_empty() {
            let Some(repo) = self.repos.repo.find_by_id(repo_id).await? else {
                return Ok(ScanCounts {
                    failed: 1,
                    ..Default::default()
                });
            };
            let Some(head_id) = repo.head_commit_id.as_deref() else {
                return Ok(ScanCounts {
                    failed: 1,
                    ..Default::default()
                });
            };
            let Some(head) = self
                .repos
                .commit
                .find_by_repo_and_commit_id(repo_id, head_id)
                .await?
            else {
                return Ok(ScanCounts {
                    failed: 1,
                    ..Default::default()
                });
            };
            resolved_fs_id =
                crate::fs::core::tree::resolve_fs_id(&self.repos, repo_id, &head.root_id, path)
                    .await?;
            resolved_fs_id.as_str()
        } else {
            fs_id
        };

        let filename = path.rsplit_once('/').map(|(_, name)| name).unwrap_or(path);
        let ext = extension_of(filename);
        let source = source_for(&ext);

        let previous = self.repos.face_tags.find_scan(repo_id, path).await?;
        if let Some(previous) = &previous {
            if !needs_scan(
                previous,
                fs_id,
                &ext,
                true,
                crate::sandbox::worker::helper().is_some(),
            ) {
                return Ok(ScanCounts {
                    skipped: 1,
                    ..Default::default()
                });
            }
            if previous.fs_id != fs_id {
                // The content changed; a user correction about the old pixels
                // must not suppress the new result.
                self.repos
                    .face_tags
                    .clear_exclusions_for_path(repo_id, path)
                    .await?;
            }
        }

        if source == ImageSource::NotAnImage {
            return self.finish_skip(repo_id, path, fs_id, "not an image").await;
        }

        let file_data = match read_fs_file_data(&self.repos, repo_id, fs_id).await {
            Ok(data) => data,
            Err(error) => {
                tracing::debug!("face-tags {path}: cannot read object: {error}");
                return Ok(ScanCounts {
                    failed: 1,
                    ..Default::default()
                });
            }
        };
        let size = file_data.size.max(0) as u64;
        if size == 0 || size > MAX_IMAGE_BYTES as u64 {
            return self
                .finish_skip(repo_id, path, fs_id, "image over the size cap")
                .await;
        }

        let options = DetectOptions {
            min_score: config.face_tags.min_score,
            min_face_size: config.face_tags.min_size as f32,
        };
        // The detector's own gate, not the image worker's: a thumbnail burst must
        // not starve scans, and a scan must not hold an image permit. It is held
        // across the decode too, so one HEIC/AVIF scan costs one slot end to end.
        let _permit = faces::acquire_face_permit()
            .await
            .map_err(AppError::internal)?;

        let outcome = match source {
            ImageSource::InProcess => {
                self.scan_in_process(repo_id, path, &file_data, options)
                    .await
            }
            ImageSource::Media => {
                self.scan_decoded_still(repo_id, &ext, &file_data, options)
                    .await
            }
            ImageSource::NotAnImage => unreachable!("the not-an-image case returned above"),
        };

        match outcome {
            ScanOutcome::Count(summary) => {
                let count = summary.face_count;
                // Tags first, then the "indexed" row: a wait on the row must not
                // observe a completed scan whose tags are still being written.
                if let Err(error) = self.apply_tags(repo_id, path, count).await {
                    tracing::warn!("face-tags {path}: cannot materialise tags: {error}");
                    self.record_failure(repo_id, path, fs_id, "cannot materialise tags")
                        .await?;
                    return Ok(ScanCounts {
                        failed: 1,
                        ..Default::default()
                    });
                }
                self.repos
                    .face_tags
                    .upsert_scan(
                        repo_id,
                        &ScanRecord {
                            file_path: path.to_string(),
                            fs_id: fs_id.to_string(),
                            model_version: MODEL_VERSION as i32,
                            status: "indexed".to_string(),
                            reason: None,
                            face_count: count as i32,
                            max_score: Some(summary.max_score),
                            attempted_at: chrono::Utc::now().timestamp(),
                        },
                    )
                    .await?;
                if let Some(ctx) = ctx {
                    ctx.set_detail("faces", serde_json::json!(count));
                }
                Ok(ScanCounts {
                    indexed: 1,
                    ..Default::default()
                })
            }
            ScanOutcome::Skip(reason) => self.finish_skip(repo_id, path, fs_id, &reason).await,
            ScanOutcome::Retry(reason) => {
                tracing::warn!("face-tags {path}: worker refused: {reason}");
                self.record_failure(repo_id, path, fs_id, &reason).await?;
                Ok(ScanCounts {
                    failed: 1,
                    ..Default::default()
                })
            }
        }
    }

    /// Materialise "no tags" and record why this file is skipped.
    ///
    /// Tags first, then the row, for the same reason the indexed path does it: the
    /// row is what a reader waits on.
    async fn finish_skip(
        &self,
        repo_id: &str,
        path: &str,
        fs_id: &str,
        reason: &str,
    ) -> Result<ScanCounts, AppError> {
        if let Err(error) = self.apply_tags(repo_id, path, 0).await {
            tracing::warn!("face-tags {path}: cannot materialise tags: {error}");
            self.record_failure(repo_id, path, fs_id, "cannot materialise tags")
                .await?;
            return Ok(ScanCounts {
                failed: 1,
                ..Default::default()
            });
        }
        self.record_skip(repo_id, path, fs_id, reason).await?;
        Ok(ScanCounts {
            skipped: 1,
            ..Default::default()
        })
    }

    /// Scan bytes the `image` crate decodes, straight from the block store.
    async fn scan_in_process(
        &self,
        repo_id: &str,
        path: &str,
        file_data: &FsFileData,
        options: DetectOptions,
    ) -> ScanOutcome {
        let data = match Downloader::read_file_limited_from_blocks(
            repo_id,
            &self.block_store,
            &file_data.block_ids,
            file_data.size,
            None,
            MAX_IMAGE_BYTES,
        )
        .await
        {
            Ok(data) => data,
            Err(error) => {
                tracing::warn!("face-tags {path}: cannot read content: {error}");
                return ScanOutcome::Retry("cannot read content".to_string());
            }
        };
        match tokio::task::spawn_blocking(move || faces::detect(&data, options)).await {
            Ok(Ok(summary)) => ScanOutcome::Count(summary),
            Ok(Err(why)) => outcome_for_failure(why),
            Err(join) => ScanOutcome::Retry(format!("face-tag worker task failed: {join}")),
        }
    }

    /// Scan a HEIC/HEIF/AVIF file: decode one still through the media profile, then
    /// run the detector on it.
    ///
    /// The still is the only thing either child sees of the file: ffmpeg reads the
    /// scratch file the media grant names, and the face worker reads the JPEG over its
    /// stdin. Nothing decodes user bytes in this process.
    async fn scan_decoded_still(
        &self,
        repo_id: &str,
        ext: &str,
        file_data: &FsFileData,
        options: DetectOptions,
    ) -> ScanOutcome {
        let Some(ffmpeg) = crate::sandbox::worker::helper() else {
            return ScanOutcome::Skip(SKIP_NO_FFMPEG.to_string());
        };
        let scratch = match crate::service::fs::scratch::write_blocks(
            &self.config.get().storage.temp_dir.join("face_scans"),
            "face-scan",
            ext,
            repo_id,
            &file_data.block_ids,
            &self.block_store,
            MAX_IMAGE_BYTES as u64,
        )
        .await
        {
            Ok(Some(scratch)) => scratch,
            Ok(None) => return ScanOutcome::Retry("cannot write the decode source".to_string()),
            Err(error) => {
                return ScanOutcome::Retry(format!("cannot write the decode source: {error}"));
            }
        };
        let source = scratch.granted_path();
        let ffmpeg = ffmpeg.to_path_buf();
        let decoded = tokio::task::spawn_blocking(move || {
            let frame = match media::still(&source, &ffmpeg) {
                Ok(frame) => frame,
                Err(StillError::Unavailable(why)) | Err(StillError::Failed(why)) => {
                    return ScanOutcome::Retry(format!("HEIC/AVIF decode unavailable: {why}"));
                }
                Err(StillError::Refused(why)) => {
                    return ScanOutcome::Skip(format!("HEIC/AVIF decode refused: {why}"));
                }
            };
            match faces::detect(&frame, options) {
                Ok(summary) => ScanOutcome::Count(summary),
                Err(why) => outcome_for_failure(why),
            }
        })
        .await;
        // Dropping the guard removes the file; it has to outlive the child, which the
        // await above guarantees.
        drop(scratch);
        decoded.unwrap_or_else(|join| {
            ScanOutcome::Retry(format!("face-tag worker task failed: {join}"))
        })
    }

    async fn record_skip(
        &self,
        repo_id: &str,
        path: &str,
        fs_id: &str,
        reason: &str,
    ) -> Result<(), AppError> {
        self.repos
            .face_tags
            .upsert_scan(
                repo_id,
                &ScanRecord {
                    file_path: path.to_string(),
                    fs_id: fs_id.to_string(),
                    model_version: MODEL_VERSION as i32,
                    status: "skipped".to_string(),
                    reason: Some(reason.to_string()),
                    face_count: 0,
                    max_score: None,
                    attempted_at: chrono::Utc::now().timestamp(),
                },
            )
            .await
    }

    async fn record_failure(
        &self,
        repo_id: &str,
        path: &str,
        fs_id: &str,
        reason: &str,
    ) -> Result<(), AppError> {
        self.repos
            .face_tags
            .upsert_scan(
                repo_id,
                &ScanRecord {
                    file_path: path.to_string(),
                    fs_id: fs_id.to_string(),
                    model_version: MODEL_VERSION as i32,
                    status: "failed".to_string(),
                    reason: Some(reason.to_string()),
                    face_count: 0,
                    max_score: None,
                    attempted_at: chrono::Utc::now().timestamp(),
                },
            )
            .await
    }

    /// Materialise the automatic tags for one path, preserving hand-made tags.
    async fn apply_tags(
        &self,
        repo_id: &str,
        path: &str,
        face_count: usize,
    ) -> Result<(), AppError> {
        let exclusions = self.repos.face_tags.list_exclusions(repo_id, path).await?;
        let language = self.config.get().ui.default_language.clone();

        let mut auto_ids = Vec::new();
        for label in labels::for_count(face_count) {
            if exclusions.iter().any(|key| key == label.key) {
                continue;
            }
            let Some(tag) = self.ensure_tag(repo_id, label, &language).await? else {
                continue;
            };
            auto_ids.push(tag);
        }

        // Keep manual tags exactly as they are; replace only the automatic set.
        let existing_links = self
            .repos
            .file_tag
            .find_by_repo_and_path(repo_id, path)
            .await?;
        let existing_ids: Vec<i32> = existing_links.iter().map(|link| link.repo_tag_id).collect();
        let existing_tags = self.repos.repo_tag.find_by_ids(&existing_ids).await?;
        let mut ids: Vec<i32> = existing_tags
            .iter()
            .filter(|tag| tag.source == "manual")
            .map(|tag| tag.id)
            .collect();
        ids.extend(auto_ids);
        ids.sort_unstable();
        ids.dedup();
        self.repos.file_tag.set_for_path(repo_id, path, &ids).await
    }

    async fn ensure_tag(
        &self,
        repo_id: &str,
        label: &LabelDef,
        language: &str,
    ) -> Result<Option<i32>, AppError> {
        if let Some(tag) = self
            .repos
            .repo_tag
            .find_by_repo_and_label_key(repo_id, label.key)
            .await?
        {
            return Ok(Some(tag.id));
        }
        let name = labels::display_name(label, language).to_string();
        if let Some(tag) = self
            .repos
            .repo_tag
            .find_by_repo_and_name(repo_id, &name)
            .await?
        {
            if tag.label_key.is_none() {
                self.repos
                    .repo_tag
                    .set_label_key(tag.id, Some(label.key.to_string()))
                    .await?;
            }
            return Ok(Some(tag.id));
        }
        let created = self
            .repos
            .repo_tag
            .create_many(
                repo_id,
                &[TagInput::automatic(
                    name,
                    label.color.to_string(),
                    label.key.to_string(),
                )],
            )
            .await?;
        Ok(created.first().map(|tag| tag.id))
    }

    /// One low-load pass over existing images. The cursor walks repositories so
    /// a single huge library cannot starve the others forever.
    pub async fn backfill_pass(
        &self,
        ctx: Option<&JobContext>,
        cursor: &Arc<Mutex<usize>>,
    ) -> Result<BackfillReport, AppError> {
        let config = self.config.get();
        if !config.face_tags.enabled || !config.face_tags.backfill_enabled {
            return Ok(BackfillReport::default());
        }
        let repos = self.repos.repo.find_all().await?;
        if repos.is_empty() {
            return Ok(BackfillReport::default());
        }
        let start = {
            let mut cursor = cursor
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let start = *cursor % repos.len();
            *cursor = (start + 1) % repos.len();
            start
        };
        let deadline = Instant::now() + BACKFILL_BUDGET;
        let mut report = BackfillReport::default();
        for repo in repos.iter().cycle().skip(start).take(repos.len()) {
            if Instant::now() >= deadline {
                break;
            }
            if let Some(ctx) = ctx {
                ctx.checkpoint()
                    .await
                    .map_err(|_| AppError::OperationFailed("face-tag backfill cancelled".into()))?;
            }
            let Some(head_id) = repo.head_commit_id.as_deref() else {
                continue;
            };
            let Some(head) = self
                .repos
                .commit
                .find_by_repo_and_commit_id(&repo.id, head_id)
                .await?
            else {
                continue;
            };
            if head.root_id == infra::common::EMPTY_SHA1 {
                continue;
            }
            report.repos_scanned += 1;
            let entries = crate::indexer::collect_file_entries_under(
                &self.repos,
                &repo.id,
                &head.root_id,
                "",
            )
            .await?;
            report.files_checked += entries.len() as u64;
            for entry in entries {
                if Instant::now() >= deadline {
                    break;
                }
                let ext = std::path::Path::new(&entry.path)
                    .extension()
                    .and_then(|ext| ext.to_str())
                    .map(str::to_ascii_lowercase)
                    .unwrap_or_default();
                if !crate::thumbnail_util::is_thumbnail_image_ext(&ext) {
                    continue;
                }
                if let Some(scan) = self
                    .repos
                    .face_tags
                    .find_scan(&repo.id, &entry.path)
                    .await?
                    && !needs_scan(
                        &scan,
                        &entry.fs_id,
                        &ext,
                        false,
                        crate::sandbox::worker::helper().is_some(),
                    )
                {
                    continue;
                }
                let result = self
                    .scan_one(ctx, &repo.id, &entry.path, &entry.fs_id)
                    .await?;
                report.counts.add(result);
            }
        }
        Ok(report)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use infra::entity::face_tag_scan::Model;

    fn row(
        status: &str,
        reason: Option<&str>,
        fs_id: &str,
        model_version: i32,
        attempted_at: i64,
    ) -> Model {
        Model {
            id: 1,
            repo_id: "repo".to_string(),
            file_path: "/a.jpg".to_string(),
            fs_id: fs_id.to_string(),
            model_version,
            status: status.to_string(),
            reason: reason.map(str::to_string),
            face_count: 0,
            max_score: None,
            attempted_at,
        }
    }

    /// A refused decode is recorded as a skip, so the backfill stops asking; a
    /// failed child stays retryable.
    #[test]
    fn a_refused_decode_is_a_skip_and_a_failed_run_is_not() {
        let skipped = outcome_for_failure(FaceError::Refused(
            "image decoder creation failed: Image size exceeds limit".to_string(),
        ));
        let ScanOutcome::Skip(reason) = skipped else {
            panic!("a refusal must be a skip");
        };
        assert!(reason.starts_with("image cannot be decoded: "), "{reason}");
        assert!(reason.contains("Image size exceeds limit"), "{reason}");

        let retry = outcome_for_failure(FaceError::Retry("detector failed".to_string()));
        assert!(matches!(retry, ScanOutcome::Retry(why) if why == "detector failed"));
    }

    /// The extension decides which decoder the bytes need; the two lists are the
    /// thumbnail service's, so a format it can show is a format the scan can read.
    #[test]
    fn a_format_names_the_decoder_it_needs() {
        for ext in [
            "png", "jpg", "jpeg", "gif", "webp", "bmp", "ico", "tiff", "tif",
        ] {
            assert_eq!(source_for(ext), ImageSource::InProcess, "{ext}");
        }
        for ext in ["heic", "heif", "avif"] {
            assert_eq!(source_for(ext), ImageSource::Media, "{ext}");
        }
        for ext in ["", "mp4", "pdf", "txt"] {
            assert_eq!(source_for(ext), ImageSource::NotAnImage, "{ext}");
        }
        assert_eq!(extension_of("/a/B/Photo.HEIC"), "heic");
        assert_eq!(extension_of("/a/noext"), "");
    }

    /// A row is re-scanned when its content or the model moved, and only then.
    #[test]
    fn a_row_is_rescanned_when_its_content_or_model_moved() {
        let indexed = row("indexed", None, "abc", MODEL_VERSION as i32, 0);
        assert!(!needs_scan(&indexed, "abc", "jpg", true, false));
        assert!(
            needs_scan(&indexed, "def", "jpg", true, false),
            "new content is a new scan"
        );
        assert!(
            needs_scan(
                &row("indexed", None, "abc", 0, 0),
                "abc",
                "jpg",
                true,
                false
            ),
            "an older model version is a new scan"
        );
    }

    /// A failure waits for the retry window in a scan job; the backfill pass is
    /// already rate-limited by its own idle window and retries one it meets.
    #[test]
    fn a_failed_row_waits_for_the_hour_in_a_scan_job_only() {
        let now = chrono::Utc::now().timestamp();
        let stale = row(
            "failed",
            Some("worker refused"),
            "abc",
            MODEL_VERSION as i32,
            now - FAILED_RETRY_AFTER_SECS - 1,
        );
        let fresh = row(
            "failed",
            Some("worker refused"),
            "abc",
            MODEL_VERSION as i32,
            now,
        );
        assert!(needs_scan(&stale, "abc", "jpg", true, false));
        assert!(!needs_scan(&fresh, "abc", "jpg", true, false));
        assert!(needs_scan(&fresh, "abc", "jpg", false, false));
    }

    /// A HEIC/AVIF row skipped for want of a decoder is picked up once one exists;
    /// any other skip is a verdict on the bytes and is left alone.
    #[test]
    fn a_heic_skip_is_retried_once_a_decoder_exists() {
        for reason in RETRY_WHEN_DECODABLE {
            let previous = row("skipped", Some(reason), "abc", MODEL_VERSION as i32, 0);
            assert!(
                needs_scan(&previous, "abc", "heic", true, true),
                "{reason} is a capability skip"
            );
            assert!(
                !needs_scan(&previous, "abc", "heic", true, false),
                "a host with no decoder must not churn on the row"
            );
            // The same reason on a format the image crate already decoded is not
            // the capability case.
            assert!(!needs_scan(&previous, "abc", "png", true, true));
        }
        let refused = row(
            "skipped",
            Some("HEIC/AVIF decode refused: no frame"),
            "abc",
            MODEL_VERSION as i32,
            0,
        );
        assert!(!needs_scan(&refused, "abc", "heic", true, true));
    }
}
