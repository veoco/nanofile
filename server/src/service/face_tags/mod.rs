//! The face-tag scan pipeline: read an image, ask the confined worker for a
//! face count, and materialise the `人物`/`单人`/`合影` tags.
//!
//! The only thing persisted about a face is its count and the best score. There
//! is no embedding, face crop or identity in this module.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use futures::StreamExt;
use tokio::sync::Semaphore;

use crate::face_tags::MODEL_VERSION;
use crate::face_tags::labels::{self, LabelDef};
use crate::fs::core::download::Downloader;
use crate::fs::core::tree::read_fs_file_data;
use crate::repository::Repositories;
use crate::repository::face_tags::ScanRecord;
use crate::repository::repo_tag::TagInput;
use crate::sandbox::jobs::images;
use crate::tasks::context::JobContext;
use base::error::AppError;
use infra::storage::DynBlockStorage;

const MAX_IMAGE_BYTES: usize = 32 * 1024 * 1024;
const BACKFILL_BUDGET: Duration = Duration::from_secs(20);
const FAILED_RETRY_AFTER_SECS: i64 = 3600;
const CONCURRENCY: usize = 2;

static GATE: std::sync::OnceLock<Arc<Semaphore>> = std::sync::OnceLock::new();

fn gate() -> &'static Arc<Semaphore> {
    GATE.get_or_init(|| Arc::new(Semaphore::new(CONCURRENCY)))
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

        let previous = self.repos.face_tags.find_scan(repo_id, path).await?;
        if let Some(previous) = &previous {
            let same_content = previous.fs_id == fs_id;
            let current_model = previous.model_version == MODEL_VERSION as i32;
            let retry_ready = previous.status != "failed"
                || chrono::Utc::now().timestamp() - previous.attempted_at
                    >= FAILED_RETRY_AFTER_SECS;
            if same_content && current_model && (!retry_ready || previous.status != "failed") {
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

        let filename = path.rsplit_once('/').map(|(_, name)| name).unwrap_or(path);
        let ext = std::path::Path::new(filename)
            .extension()
            .and_then(|ext| ext.to_str())
            .map(str::to_ascii_lowercase)
            .unwrap_or_default();
        // The image crate does not decode HEIC/HEIF/AVIF; those need the media
        // worker and are deliberately skipped for now rather than retried as
        // failures every hour.
        if matches!(ext.as_str(), "heic" | "heif" | "avif") {
            self.record_skip(repo_id, path, fs_id, "HEIC/AVIF face scan not supported")
                .await?;
            if let Err(error) = self.apply_tags(repo_id, path, 0).await {
                tracing::warn!("face-tags {path}: cannot materialise tags: {error}");
                self.record_failure(repo_id, path, fs_id, "cannot materialise tags")
                    .await?;
                return Ok(ScanCounts {
                    failed: 1,
                    ..Default::default()
                });
            }
            return Ok(ScanCounts {
                skipped: 1,
                ..Default::default()
            });
        }
        if !crate::thumbnail_util::is_thumbnail_image_ext(&ext) {
            self.repos
                .face_tags
                .upsert_scan(
                    repo_id,
                    &ScanRecord {
                        file_path: path.to_string(),
                        fs_id: fs_id.to_string(),
                        model_version: MODEL_VERSION as i32,
                        status: "skipped".to_string(),
                        reason: Some("not an image".to_string()),
                        face_count: 0,
                        max_score: None,
                        attempted_at: chrono::Utc::now().timestamp(),
                    },
                )
                .await?;
            if let Err(error) = self.apply_tags(repo_id, path, 0).await {
                tracing::warn!("face-tags {path}: cannot materialise tags: {error}");
                self.record_failure(repo_id, path, fs_id, "cannot materialise tags")
                    .await?;
                return Ok(ScanCounts {
                    failed: 1,
                    ..Default::default()
                });
            }
            return Ok(ScanCounts {
                skipped: 1,
                ..Default::default()
            });
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
            self.record_skip(repo_id, path, fs_id, "image over the size cap")
                .await?;
            if let Err(error) = self.apply_tags(repo_id, path, 0).await {
                tracing::warn!("face-tags {path}: cannot materialise tags: {error}");
                self.record_failure(repo_id, path, fs_id, "cannot materialise tags")
                    .await?;
                return Ok(ScanCounts {
                    failed: 1,
                    ..Default::default()
                });
            }
            return Ok(ScanCounts {
                skipped: 1,
                ..Default::default()
            });
        }

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
                self.record_failure(repo_id, path, fs_id, "cannot read content")
                    .await?;
                return Ok(ScanCounts {
                    failed: 1,
                    ..Default::default()
                });
            }
        };

        let options = crate::face_tags::DetectOptions {
            min_score: config.face_tags.min_score,
            min_face_size: config.face_tags.min_size as f32,
        };
        let _permit = gate()
            .clone()
            .acquire_owned()
            .await
            .map_err(|e| AppError::internal(format!("face-tag gate closed: {e}")))?;
        let result = tokio::task::spawn_blocking(move || images::face_tags(&data, options))
            .await
            .map_err(|e| AppError::internal(format!("face-tag worker task failed: {e}")))?;

        match result {
            Ok(summary) => {
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
            Err(error) => {
                tracing::warn!("face-tags {path}: worker refused: {error}");
                self.record_failure(repo_id, path, fs_id, &error).await?;
                Ok(ScanCounts {
                    failed: 1,
                    ..Default::default()
                })
            }
        }
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
                    && scan.fs_id == entry.fs_id
                    && scan.model_version == MODEL_VERSION as i32
                    && scan.status != "failed"
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
