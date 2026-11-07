//! The full-text index pipeline: read a file, extract its text, record the
//! result and the state that says what was recorded.
//!
//! Every path that wants a file indexed goes through here — the per-upload task,
//! a sync batch, the manual re-index endpoint and the low-load backfill — and
//! they differ only in where the file list comes from and whether the work may
//! park under load.
//!
//! The index is derived data: a file that cannot be read is counted and logged,
//! never a reason to fail the request that scheduled the work. A *backfill* must
//! not touch a document a client pinned by hand (`/api2/index-file-text/`) or
//! re-read a file it already processed. An *explicit* request refreshes even a
//! pinned document.

use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use futures::StreamExt;
use tokio::sync::Semaphore;

use crate::indexer::extract;
use crate::indexer::{DocMeta, TextIndexer, collect_file_entries_under};
use crate::repository::Repositories;
use crate::sandbox::worker;
use crate::tasks::context::JobContext;
use crate::tasks::run::JobFailure;
use crate::tasks::spec::JobKey;
use crate::tasks::{Origin, TaskSystem};
use base::error::AppError;
use infra::storage::DynBlockStorage;

const CONCURRENCY: usize = 8;

const BACKFILL_BUDGET: Duration = Duration::from_secs(20);

pub const BATCH_FILES: usize = 256;

/// Returned when a pass stopped at a checkpoint rather than finishing; the job
/// body maps it back onto a cancellation, as the repository reindex does.
pub const ABORTED: &str = "indexing stopped at a checkpoint";

/// How many documents may be parsed at once, process-wide: one file can hold its
/// whole document in memory on top of its own caches and a batch keeps
/// [`CONCURRENCY`] files in flight, so this bounds large parses. Callers queue.
const STRUCTURED_CONCURRENCY: usize = 2;

static STRUCTURED_GATE: OnceLock<Arc<Semaphore>> = OnceLock::new();

fn structured_gate() -> &'static Arc<Semaphore> {
    STRUCTURED_GATE.get_or_init(|| Arc::new(Semaphore::new(STRUCTURED_CONCURRENCY)))
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct IndexTarget {
    /// Repo-absolute path with a leading `/`.
    pub path: String,
    /// The file's content identity, when the caller already knows it. A caller that
    /// does not (the manual endpoint) leaves this unset.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fs_id: Option<String>,
}

impl IndexTarget {
    pub fn new(path: impl Into<String>, fs_id: Option<String>) -> Self {
        Self {
            path: path.into(),
            fs_id,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct IndexCounts {
    pub indexed: u64,
    /// Files examined and deliberately not indexed (binary, unsupported).
    pub skipped: u64,
    /// Files whose read or extraction failed; the backfill retries them.
    pub failed: u64,
    pub gone: u64,
}

impl IndexCounts {
    pub fn total(&self) -> u64 {
        self.indexed + self.skipped + self.failed + self.gone
    }

    pub fn add(&mut self, other: IndexCounts) {
        self.indexed += other.indexed;
        self.skipped += other.skipped;
        self.failed += other.failed;
        self.gone += other.gone;
    }
}

/// Why a batch is being indexed, which decides what "already done" means.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IndexMode {
    /// The caller is saying these paths changed. A document a client pinned by
    /// hand is refreshed; an unchanged document is still skipped.
    Explicit,
    /// An automatic pass. A pinned document is never touched: the client knows
    /// the format better than the extractor does.
    Backfill,
    /// Rebuild: re-read and rewrite every file, whatever the index already says.
    Force,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OneOutcome {
    Indexed,
    Skipped,
    /// The read or extraction failed; a failed document was recorded.
    Failed,
    Gone,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct BackfillReport {
    pub repos_scanned: u64,
    pub files_checked: u64,
    pub counts: IndexCounts,
}

/// The read → extract → write pipeline over one index.
#[derive(Clone)]
pub struct IndexService {
    repos: Arc<Repositories>,
    block_store: DynBlockStorage,
    indexer: TextIndexer,
    /// The live snapshot: `[ocr]` can be switched on without a restart, and the
    /// next backfill pass picks existing images up.
    config: crate::settings::RuntimeConfig,
}

impl IndexService {
    pub fn new(
        repos: Arc<Repositories>,
        block_store: DynBlockStorage,
        indexer: TextIndexer,
        config: crate::settings::RuntimeConfig,
    ) -> Self {
        Self {
            repos,
            block_store,
            indexer,
            config,
        }
    }

    pub fn indexer(&self) -> &TextIndexer {
        &self.indexer
    }

    /// Index one file, resolving its content identity from the repo tree; used by
    /// the manual single-file re-index endpoint, where the caller has a path only.
    pub async fn index_path(&self, repo_id: &str, path: &str) -> Result<OneOutcome, AppError> {
        let head_root = self.head_root(repo_id).await?;
        let target = IndexTarget::new(path, None);
        self.index_one(None, repo_id, &target, &head_root).await
    }

    /// Index the given files with bounded concurrency, checking in before each read
    /// so the pass parks while the server is busy. `mode` selects what "already
    /// done" means; see [`IndexMode`].
    pub async fn index_batch(
        &self,
        ctx: Option<&JobContext>,
        repo_id: &str,
        targets: &[IndexTarget],
        mode: IndexMode,
    ) -> Result<IndexCounts, AppError> {
        let mut counts = IndexCounts::default();
        if targets.is_empty() {
            return Ok(counts);
        }

        let head_root = match self.head_root(repo_id).await {
            Ok(root) => root,
            // A repository that has gone away is no error for a derived index.
            Err(e) => {
                tracing::debug!("cannot index {repo_id}: {e}");
                counts.gone = targets.len() as u64;
                return Ok(counts);
            }
        };

        // Drop the files this repo already has a current document for: no block read,
        // no tokenization.
        let paths: Vec<String> = targets.iter().map(|t| t.path.clone()).collect();
        let states = self.indexer.doc_states(repo_id, &paths)?;
        let now = chrono::Utc::now().timestamp();
        let ocr_on = self.config.get().ocr.enabled;
        // An image or scanned PDF that was skipped before OCR was enabled has a
        // `Skipped` state that is current at this extractor version. When OCR is
        // on, re-examine those files once; OCR then records `Indexed` or
        // `NoText`, and the pass converges instead of re-reading them forever.
        let ocr_worth_retry = |target: &IndexTarget, state: &crate::indexer::DocState| {
            ocr_on
                && (crate::ocr::is_image(&target.path) || crate::ocr::is_pdf(&target.path))
                && state.status == crate::indexer::DocStatus::Skipped
                && !state.is_manual()
        };
        let work: Vec<IndexTarget> = targets
            .iter()
            .filter(|target| {
                if mode == IndexMode::Force {
                    return true;
                }
                // Content identity unknown: the only safe answer is "look".
                let Some(fs_id) = target.fs_id.as_deref().filter(|id| !id.is_empty()) else {
                    return true;
                };
                match states.get(&target.path) {
                    // An explicit request refreshes even a hand-pinned document; a
                    // backfill leaves those alone.
                    Some(state) if mode == IndexMode::Explicit => {
                        state.is_manual()
                            || !state.is_current(fs_id, now)
                            || ocr_worth_retry(target, state)
                    }
                    Some(state) => !state.is_current(fs_id, now) || ocr_worth_retry(target, state),
                    None => true,
                }
            })
            .cloned()
            .collect();

        if work.is_empty() {
            return Ok(counts);
        }

        let width = (CONCURRENCY as f32 * ctx.map_or(1.0, JobContext::budget))
            .max(1.0)
            .round() as usize;
        let total = work.len() as u64;
        let mut done = 0u64;
        let stream = futures::stream::iter(work.into_iter().map(|target| {
            let svc = self.clone();
            let repo_id = repo_id.to_string();
            let head_root = head_root.clone();
            let ctx = ctx.cloned();
            async move {
                svc.index_one(ctx.as_ref(), &repo_id, &target, &head_root)
                    .await
            }
        }))
        .buffer_unordered(width.max(1));

        futures::pin_mut!(stream);
        while let Some(result) = stream.next().await {
            match result? {
                OneOutcome::Indexed => counts.indexed += 1,
                OneOutcome::Skipped => counts.skipped += 1,
                OneOutcome::Failed => counts.failed += 1,
                OneOutcome::Gone => counts.gone += 1,
            }
            done += 1;
            if let Some(ctx) = ctx {
                ctx.report(done, Some(total));
            }
        }
        Ok(counts)
    }

    /// Rebuild a repository's index. Every file in HEAD is read and rewritten, so
    /// `indexed` is the number of files the library holds; skipping is the
    /// *backfill*'s job, and a rebuild that skipped files would report zero.
    pub async fn reindex_repo(
        &self,
        ctx: Option<&JobContext>,
        repo_id: &str,
    ) -> Result<IndexCounts, AppError> {
        let head_root = self.head_root(repo_id).await?;
        if head_root == infra::common::EMPTY_SHA1 {
            return Ok(IndexCounts::default());
        }
        let entries = collect_file_entries_under(&self.repos, repo_id, &head_root, "").await?;
        let targets: Vec<IndexTarget> = entries
            .into_iter()
            .map(|entry| IndexTarget::new(entry.path, Some(entry.fs_id)))
            .collect();
        self.index_batch(ctx, repo_id, &targets, IndexMode::Force)
            .await
    }

    /// Rebuild the index for a moved subtree: every file below it needs a document at
    /// its new path, and the caller drops the old ones.
    pub async fn reindex_subtree(
        &self,
        ctx: Option<&JobContext>,
        repo_id: &str,
        dir_fs_id: &str,
        base_path: &str,
    ) -> Result<IndexCounts, AppError> {
        let entries =
            collect_file_entries_under(&self.repos, repo_id, dir_fs_id, base_path).await?;
        let targets: Vec<IndexTarget> = entries
            .into_iter()
            .map(|entry| IndexTarget::new(entry.path, Some(entry.fs_id)))
            .collect();
        self.index_batch(ctx, repo_id, &targets, IndexMode::Explicit)
            .await
    }

    /// Look at one repository's files and index whatever is missing or stale.
    ///
    /// The pass advances `cursor` so on a busy server every repository is reached
    /// rather than the first one being retried forever.
    pub async fn backfill_pass(
        &self,
        ctx: Option<&JobContext>,
        cursor: &Arc<Mutex<usize>>,
    ) -> Result<BackfillReport, AppError> {
        let mut report = BackfillReport::default();
        let mut repos = self.repos.repo.find_all().await?;
        repos.sort_by(|a, b| a.id.cmp(&b.id));
        if repos.is_empty() {
            return Ok(report);
        }

        let start = *cursor.lock().unwrap_or_else(|e| e.into_inner()) % repos.len();
        let deadline = Instant::now() + BACKFILL_BUDGET;

        for offset in 0..repos.len() {
            let index = (start + offset) % repos.len();
            let repo = &repos[index];
            // Advance past this repository even if the pass stops inside it.
            *cursor.lock().unwrap_or_else(|e| e.into_inner()) = (index + 1) % repos.len();

            if ctx.is_some_and(JobContext::is_cancelled) {
                return Err(AppError::OperationFailed(ABORTED.into()));
            }
            if Instant::now() >= deadline {
                break;
            }
            if repo.head_commit_id.is_none() {
                continue;
            }
            let Ok(root) = self.head_root(&repo.id).await else {
                continue;
            };
            if root == infra::common::EMPTY_SHA1 {
                continue;
            }
            report.repos_scanned += 1;

            let Ok(entries) = collect_file_entries_under(&self.repos, &repo.id, &root, "").await
            else {
                continue;
            };
            report.files_checked += entries.len() as u64;
            let targets: Vec<IndexTarget> = entries
                .into_iter()
                .map(|entry| IndexTarget::new(entry.path, Some(entry.fs_id)))
                .collect();
            report.counts.add(
                self.index_batch(ctx, &repo.id, &targets, IndexMode::Backfill)
                    .await?,
            );
        }
        Ok(report)
    }

    /// Index one file: resolve its identity, read what its format needs, record the outcome.
    async fn index_one(
        &self,
        ctx: Option<&JobContext>,
        repo_id: &str,
        target: &IndexTarget,
        head_root: &str,
    ) -> Result<OneOutcome, AppError> {
        checkpoint(ctx).await?;

        let path = &target.path;
        let filename = path.rsplit_once('/').map(|(_, name)| name).unwrap_or(path);

        // A caller that knows the fs_id (an upload, a sync diff, the tree walk) skips
        // the lookup; the manual endpoint does not have one.
        let fs_id = match target.fs_id.as_deref().filter(|id| !id.is_empty()) {
            Some(id) => id.to_string(),
            None => {
                match crate::fs::core::resolve_file_entry_with_head(
                    &self.repos,
                    repo_id,
                    head_root,
                    path,
                )
                .await
                {
                    Ok(Some((id, false, _size, _mtime))) => id,
                    Ok(_) => return Ok(OneOutcome::Gone),
                    Err(e) => {
                        tracing::debug!("indexing {path}: cannot resolve: {e}");
                        return Ok(OneOutcome::Gone);
                    }
                }
            }
        };

        let file_data = match crate::fs::core::read_fs_file_data(&self.repos, repo_id, &fs_id).await
        {
            Ok(data) => data,
            Err(e) => {
                tracing::debug!("indexing {path}: cannot read file object: {e}");
                return Ok(OneOutcome::Gone);
            }
        };

        let config = self.config.get();
        let size = file_data.size.max(0) as u64;

        // What this file is, and whether an OCR path owns it. The plan is still
        // computed for the non-OCR path; an image extension only ever reaches
        // `Sniff`.
        let plan = extract::plan(filename);
        let ocr_image = if config.ocr.enabled {
            crate::ocr::source_for(filename)
        } else {
            None
        };
        let ocr_pdf =
            config.ocr.enabled && matches!(plan, extract::Plan::Document(extract::Document::Pdf));
        let ocr_enabled = ocr_image.is_some() || ocr_pdf;
        let ocr_options = crate::ocr::OcrOptions {
            min_score: config.ocr.min_score,
            max_side: config.ocr.max_side,
        };

        if ocr_image.is_some() && (size == 0 || size > crate::ocr::MAX_IMAGE_BYTES as u64) {
            tracing::debug!("indexing {path}: image is {size} bytes, outside the OCR cap");
            self.indexer
                .mark_file_async(repo_id, path, filename, DocMeta::no_text(fs_id.as_str()))
                .await?;
            return Ok(OneOutcome::Skipped);
        }

        // A document above the cap is skipped unread: a container cannot be indexed
        // from a prefix. An image is OCR'd whole, up to the OCR cap.
        let budget = if ocr_image.is_some() {
            crate::ocr::MAX_IMAGE_BYTES
        } else {
            match extract::budget(plan, size) {
                Ok(budget) => budget,
                Err(reason) => {
                    tracing::debug!("indexing {path}: not indexable: {reason}");
                    // A PDF that is too large cannot be OCR'd either; with OCR on
                    // this is terminal, so the backfill does not re-read it forever.
                    let meta = if ocr_enabled {
                        DocMeta::no_text(fs_id.as_str())
                    } else {
                        DocMeta::skipped(fs_id.as_str())
                    };
                    self.indexer
                        .mark_file_async(repo_id, path, filename, meta)
                        .await?;
                    return Ok(OneOutcome::Skipped);
                }
            }
        };

        // A failure here is recorded as a failed document so the backfill retries
        // it after a cooldown instead of re-reading it on every pass.
        let data = match crate::fs::core::download::Downloader::read_file_limited_from_blocks(
            repo_id,
            &self.block_store,
            &file_data.block_ids,
            file_data.size,
            None,
            budget,
        )
        .await
        {
            Ok(data) => data,
            Err(e) => {
                tracing::warn!("indexing {path}: cannot read content: {e}");
                self.indexer
                    .mark_file_async(repo_id, path, filename, DocMeta::failed(fs_id.as_str()))
                    .await?;
                return Ok(OneOutcome::Failed);
            }
        };

        // A document or an image is read whole, so a short read is a transient
        // failure.
        if (matches!(plan, extract::Plan::Document(_)) || ocr_image.is_some())
            && (data.len() as u64) < size
        {
            tracing::warn!("indexing {path}: read {} of {size} bytes", data.len());
            self.indexer
                .mark_file_async(repo_id, path, filename, DocMeta::failed(fs_id.as_str()))
                .await?;
            return Ok(OneOutcome::Failed);
        }

        let data = Arc::new(data);

        let extracted = if let Some(source) = ocr_image {
            let extension = extension_of(filename);
            match self
                .ocr_extract(
                    ctx,
                    repo_id,
                    &extension,
                    &file_data,
                    Arc::clone(&data),
                    source,
                    ocr_options,
                )
                .await?
            {
                worker::Outcome::Extracted(extracted) => extracted,
                // The sandbox, the helper or a model is unavailable: an
                // environment problem, retried once the host can do the work.
                worker::Outcome::Unavailable(reason) => {
                    tracing::warn!("indexing {path}: OCR is unavailable: {reason}");
                    self.indexer
                        .mark_file_async(repo_id, path, filename, DocMeta::failed(fs_id.as_str()))
                        .await?;
                    return Ok(OneOutcome::Failed);
                }
                // The child ran and did not answer: a property of the image, so it
                // is terminal for this content rather than retried every pass.
                worker::Outcome::Failed(reason) => extract::Extracted::Unsupported(reason),
            }
        } else if ocr_pdf {
            // The text layer is extracted page by page so the indexer can ask
            // OCR only for pages that have little or no text. The document gate is
            // released before the OCR gate is taken.
            let page_data = Arc::clone(&data);
            let pages = {
                let _permit = structured_gate()
                    .clone()
                    .acquire_owned()
                    .await
                    .map_err(|e| AppError::internal(format!("document gate closed: {e}")))?;
                offload_extract(ctx, move || {
                    worker::extract(extract::Plan::PdfPageText, &page_data)
                })
                .await?
            };
            match pages {
                worker::Outcome::Extracted(extract::Extracted::Text(joined)) => {
                    let page_texts: Vec<String> =
                        joined.split('\u{1e}').map(ToString::to_string).collect();
                    let candidates: Vec<u32> = page_texts
                        .iter()
                        .enumerate()
                        .filter(|(_, text)| pdf_page_is_scan_like(text))
                        .map(|(index, _)| index as u32 + 1)
                        .collect();
                    if candidates.is_empty() {
                        extract::finish(page_texts.join("\n"))
                    } else {
                        match self
                            .ocr_extract_pdf(ctx, Arc::clone(&data), &candidates, ocr_options)
                            .await?
                        {
                            worker::Outcome::Extracted(extract::Extracted::Text(ocr_text)) => {
                                merge_pdf_pages(page_texts, &candidates, &ocr_text)
                            }
                            // The child ran and could not render/read the pages:
                            // keep whatever text layer the document had.
                            worker::Outcome::Extracted(extract::Extracted::Unsupported(reason))
                            | worker::Outcome::Failed(reason) => {
                                keep_pdf_text_or(page_texts, reason)
                            }
                            worker::Outcome::Unavailable(reason) => {
                                tracing::warn!("indexing {path}: PDF OCR is unavailable: {reason}");
                                self.indexer
                                    .mark_file_async(
                                        repo_id,
                                        path,
                                        filename,
                                        DocMeta::failed(fs_id.as_str()),
                                    )
                                    .await?;
                                return Ok(OneOutcome::Failed);
                            }
                        }
                    }
                }
                worker::Outcome::Extracted(extract::Extracted::Unsupported(reason)) => {
                    extract::Extracted::Unsupported(reason)
                }
                worker::Outcome::Unavailable(reason) => {
                    tracing::warn!("indexing {path}: {reason}");
                    self.indexer
                        .mark_file_async(repo_id, path, filename, DocMeta::failed(fs_id.as_str()))
                        .await?;
                    return Ok(OneOutcome::Failed);
                }
                worker::Outcome::Failed(reason) => extract::Extracted::Unsupported(reason),
            }
        } else if matches!(plan, extract::Plan::Document(_)) {
            // A document is parsed by a confined child process, never here: see
            // `sandbox::worker`. Spawning and waiting is synchronous and can take
            // seconds, so it runs on the blocking pool under a gate permit that bounds
            // how many children are alive; the permit is released when this branch ends.
            let doc_data = Arc::clone(&data);
            let _permit = structured_gate()
                .clone()
                .acquire_owned()
                .await
                .map_err(|e| AppError::internal(format!("document gate closed: {e}")))?;
            match offload_extract(ctx, move || worker::extract(plan, &doc_data)).await? {
                worker::Outcome::Extracted(extracted) => extracted,
                // The sandbox or the child is unavailable: an environment problem, so
                // the file is failed and the backfill retries it once the host can
                // confine the worker; deliberately *not* retried in this process.
                worker::Outcome::Unavailable(reason) => {
                    tracing::warn!("indexing {path}: {reason}");
                    self.indexer
                        .mark_file_async(repo_id, path, filename, DocMeta::failed(fs_id.as_str()))
                        .await?;
                    return Ok(OneOutcome::Failed);
                }
                // The child ran and did not answer: a property of the bytes, so the
                // document is skipped rather than retried every pass.
                worker::Outcome::Failed(reason) => extract::Extracted::Unsupported(reason),
            }
        } else {
            let owned = Arc::try_unwrap(data).unwrap_or_else(|shared| (*shared).clone());
            extract::extract(plan, owned)
        };

        match extracted {
            extract::Extracted::Text(content) => {
                self.indexer
                    .index_file_with_meta_async(
                        repo_id,
                        path,
                        filename,
                        &content,
                        DocMeta::indexed(fs_id.as_str()),
                    )
                    .await?;
                Ok(OneOutcome::Indexed)
            }
            extract::Extracted::Unsupported(reason) => {
                tracing::debug!("indexing {path}: not indexable: {reason}");
                let status = unsupported_status(reason, ocr_enabled);
                let meta = match status {
                    // The sandbox being off is the admin's choice, not a verdict
                    // on the file: a `Skipped` (or `NoText`) state would never be
                    // re-examined when the switch is turned back on. This also
                    // covers a PDF whose page-text extraction could not start
                    // because OCR is on but the sandbox is off.
                    crate::indexer::DocStatus::Failed => DocMeta::failed(fs_id.as_str()),
                    // OCR examined the image, or every page of the PDF it was
                    // asked to read, and found nothing searchable. `NoText` is
                    // current at this extractor version, so the file is not
                    // re-read by every backfill pass.
                    crate::indexer::DocStatus::NoText => DocMeta::no_text(fs_id.as_str()),
                    _ => DocMeta::skipped(fs_id.as_str()),
                };
                self.indexer
                    .mark_file_async(repo_id, path, filename, meta)
                    .await?;
                if status == crate::indexer::DocStatus::Failed {
                    Ok(OneOutcome::Failed)
                } else {
                    Ok(OneOutcome::Skipped)
                }
            }
        }
    }

    /// OCR one image in its confined child.
    ///
    /// In-process formats hand the bytes straight to the OCR profile. HEIC/HEIF/
    /// AVIF are decoded to a still through the media profile first, exactly as
    /// the face-tag scan does; only the JPEG frame reaches OCR.
    async fn ocr_extract(
        &self,
        ctx: Option<&JobContext>,
        repo_id: &str,
        extension: &str,
        file_data: &base::common::FsFileData,
        data: Arc<Vec<u8>>,
        source: crate::ocr::Source,
        options: crate::ocr::OcrOptions,
    ) -> Result<worker::Outcome, AppError> {
        match source {
            crate::ocr::Source::InProcess => {
                let _permit = crate::sandbox::jobs::ocr::acquire_ocr_permit()
                    .await
                    .map_err(AppError::internal)?;
                offload_extract(ctx, move || {
                    crate::sandbox::jobs::ocr::extract(&data, options)
                })
                .await
            }
            crate::ocr::Source::Media => {
                let Some(ffmpeg) = worker::helper() else {
                    return Ok(worker::Outcome::Unavailable(
                        "OCR needs ffmpeg for HEIC/HEIF/AVIF".to_string(),
                    ));
                };
                let scratch = match crate::service::fs::scratch::write_blocks(
                    &self.config.get().storage.temp_dir.join("ocr"),
                    "ocr",
                    extension,
                    repo_id,
                    &file_data.block_ids,
                    &self.block_store,
                    crate::ocr::MAX_IMAGE_BYTES as u64,
                )
                .await
                {
                    Ok(Some(scratch)) => scratch,
                    Ok(None) => return Ok(worker::Outcome::Failed(extract::reason::FAILED)),
                    Err(error) => return Err(error),
                };
                let source = scratch.granted_path();
                let ffmpeg = ffmpeg.to_path_buf();
                let decoded = tokio::task::spawn_blocking(move || {
                    match crate::sandbox::jobs::media::still(&source, &ffmpeg) {
                        Ok(frame) => Ok(frame),
                        Err(crate::sandbox::jobs::media::StillError::Unavailable(why))
                        | Err(crate::sandbox::jobs::media::StillError::Failed(why)) => {
                            Err(worker::Outcome::Unavailable(format!(
                                "HEIC/AVIF decode unavailable: {why}"
                            )))
                        }
                        Err(crate::sandbox::jobs::media::StillError::Refused(why)) => {
                            tracing::debug!("OCR still refused: {why}");
                            Err(worker::Outcome::Extracted(extract::Extracted::Unsupported(
                                extract::reason::NOT_TEXT,
                            )))
                        }
                    }
                })
                .await;
                // Dropping the guard removes the file; the media child is gone.
                drop(scratch);
                let frame = match decoded {
                    Ok(Ok(frame)) => frame,
                    Ok(Err(outcome)) => return Ok(outcome),
                    Err(join) => {
                        return Err(AppError::internal(format!(
                            "OCR media decode task failed: {join}"
                        )));
                    }
                };
                let _permit = crate::sandbox::jobs::ocr::acquire_ocr_permit()
                    .await
                    .map_err(AppError::internal)?;
                offload_extract(ctx, move || {
                    crate::sandbox::jobs::ocr::extract(&frame, options)
                })
                .await
            }
        }
    }

    /// Ask the confined OCR child to render and read selected PDF pages.
    ///
    /// The returned `Text` separates page texts with `\u{1e}`, in the order of
    /// `pages`; the indexer merges them back by page number.
    async fn ocr_extract_pdf(
        &self,
        ctx: Option<&JobContext>,
        data: Arc<Vec<u8>>,
        pages: &[u32],
        options: crate::ocr::OcrOptions,
    ) -> Result<worker::Outcome, AppError> {
        let _permit = crate::sandbox::jobs::ocr::acquire_ocr_permit()
            .await
            .map_err(AppError::internal)?;
        let pages = pages.to_vec();
        offload_extract(ctx, move || {
            crate::sandbox::jobs::ocr::extract_pdf(data.as_slice(), &pages, options)
        })
        .await
    }

    async fn head_root(&self, repo_id: &str) -> Result<String, AppError> {
        let repo = self
            .repos
            .repo
            .find_by_id(repo_id)
            .await?
            .ok_or_else(|| AppError::NotFound("repo not found".into()))?;
        let commit_id = repo
            .head_commit_id
            .ok_or_else(|| AppError::NotFound("repo has no commits".into()))?;
        let commit = self
            .repos
            .commit
            .find_by_id(&commit_id)
            .await?
            .ok_or_else(|| AppError::NotFound("head commit not found".into()))?;
        Ok(commit.root_id)
    }
}

/// A page with fewer non-whitespace characters than this is treated as a scan.
///
/// There is no reliable PDF-level "scanned" flag: an image-only scan has no text
/// operators, while a digital document has many. A page below this threshold is
/// rendered and read by OCR; a page at or above it keeps its text layer.
const PDF_PAGE_TEXT_MIN_CHARS: usize = 32;

/// Whether a page's text layer is small enough that the page is probably a scan.
fn pdf_page_is_scan_like(text: &str) -> bool {
    text.chars()
        .filter(|character| !character.is_whitespace())
        .count()
        < PDF_PAGE_TEXT_MIN_CHARS
}

/// Merge OCR text back into the page texts, then apply the document text rules.
///
/// `ocr` has one `\u{1e}`-separated field per candidate page, in the same order.
/// An empty OCR field keeps that page's text layer; a page outside the text
/// layer is ignored.
fn merge_pdf_pages(mut pages: Vec<String>, candidates: &[u32], ocr: &str) -> extract::Extracted {
    let ocr_pages: Vec<&str> = ocr.split('\u{1e}').collect();
    for (slot, &page_number) in candidates.iter().enumerate() {
        let Some(text) = ocr_pages.get(slot) else {
            continue;
        };
        if text.trim().is_empty() {
            continue;
        }
        let Some(page) = page_number
            .checked_sub(1)
            .map(|number| number as usize)
            .and_then(|index| pages.get_mut(index))
        else {
            continue;
        };
        *page = (*text).to_string();
    }
    extract::finish(pages.join("\n"))
}

/// The state an unsupported extraction records.
///
/// The sandbox being off is an environment choice, not a verdict on the bytes,
/// so it outranks the OCR-on rule: turning the sandbox back on must re-examine
/// the file, including a PDF whose page text could not be read because the
/// document worker never started.
fn unsupported_status(reason: &'static str, ocr_enabled: bool) -> crate::indexer::DocStatus {
    if reason == extract::reason::SANDBOX_OFF {
        crate::indexer::DocStatus::Failed
    } else if ocr_enabled {
        crate::indexer::DocStatus::NoText
    } else {
        crate::indexer::DocStatus::Skipped
    }
}

/// Keep the text layer when OCR could not run on the candidate pages.
///
/// The fallback reason is used only when the text layer has nothing either; the
/// caller's OCR-on rule then records `NoText`.
fn keep_pdf_text_or(pages: Vec<String>, reason: &'static str) -> extract::Extracted {
    match extract::finish(pages.join("\n")) {
        extract::Extracted::Text(text) => extract::Extracted::Text(text),
        extract::Extracted::Unsupported(_) => extract::Extracted::Unsupported(reason),
    }
}

/// The lowercased extension of a path, without the dot.
fn extension_of(path: &str) -> String {
    path.rsplit_once('.')
        .map(|(_, extension)| extension.to_ascii_lowercase())
        .unwrap_or_default()
}

/// Ask the run to stop, if there is a run to ask. Every pipeline entry point uses
/// this, so a cancellation and a shutdown both surface as [`ABORTED`].
pub(crate) async fn checkpoint(ctx: Option<&JobContext>) -> Result<(), AppError> {
    let Some(ctx) = ctx else {
        return Ok(());
    };
    ctx.checkpoint().await.map_err(map_failure)
}

/// Map a job failure onto this pipeline's error. A cancellation and a timeout are
/// terminal but not failures, so they surface as [`ABORTED`].
fn map_failure(failure: JobFailure) -> AppError {
    match failure {
        JobFailure::Cancelled | JobFailure::TimedOut => AppError::OperationFailed(ABORTED.into()),
        JobFailure::App(e) => e,
    }
}

/// Run one document through the extraction worker, on the blocking pool.
///
/// `JobContext::run_blocking` checkpoints *before* taking a blocking thread, so a
/// job parked under load stops replenishing the pool; the manual endpoint has no
/// context and gets a plain blocking task. The worker runs inside the extractor's
/// panic boundary: one unreadable file never fails the run that carried it.
async fn offload_extract<F>(ctx: Option<&JobContext>, f: F) -> Result<worker::Outcome, AppError>
where
    F: FnOnce() -> worker::Outcome + Send + 'static,
{
    let run = move || {
        extract::guard(
            "index",
            |_| worker::Outcome::Failed(extract::reason::PANIC),
            f,
        )
    };
    match ctx {
        Some(ctx) => ctx.run_blocking(run).await.map_err(map_failure),
        None => tokio::task::spawn_blocking(run)
            .await
            .map_err(|e| AppError::internal(format!("extraction task failed: {e}"))),
    }
}

/// Schedules index work on the task system.
///
/// Call sites describe *what changed*, and the job body — which captured this
/// generation's resources — does the work, so an upload pays no block read on the
/// request path.
#[derive(Clone)]
pub struct IndexScheduler {
    tasks: Arc<TaskSystem>,
    index_enabled: bool,
    face_tags_enabled: bool,
}

impl IndexScheduler {
    pub fn new(tasks: Arc<TaskSystem>, index_enabled: bool, face_tags_enabled: bool) -> Self {
        Self {
            tasks,
            index_enabled,
            face_tags_enabled,
        }
    }

    /// Whether scheduling can do anything; callers skip building the file list
    /// when it cannot.
    pub fn is_enabled(&self) -> bool {
        self.index_enabled || self.face_tags_enabled
    }

    /// Submit index work for `targets`, split into bounded runs.
    ///
    /// Best effort: the index is derived data and the backfill re-discovers
    /// anything a dropped submission missed, so a scheduling failure is logged.
    pub async fn schedule(
        &self,
        repo_id: &str,
        targets: Vec<IndexTarget>,
        reason: &'static str,
    ) -> usize {
        if targets.is_empty() {
            return 0;
        }
        let mut submitted = 0;
        for chunk in targets.chunks(BATCH_FILES) {
            let files: Vec<&IndexTarget> = chunk.iter().collect();
            if self.index_enabled {
                let params = serde_json::json!({
                    "repo_id": repo_id,
                    "files": files,
                    "reason": reason,
                });
                let summary = format!("Index {} file(s)", chunk.len());
                match self
                    .tasks
                    .submit_system(Origin::Schedule, JobKey::IndexFiles, params, summary)
                    .await
                {
                    Ok(_) => submitted += chunk.len(),
                    Err(e) => {
                        tracing::debug!("could not schedule indexing for {repo_id}: {e}");
                    }
                }
            }
            if self.face_tags_enabled {
                let images: Vec<&IndexTarget> = chunk
                    .iter()
                    .filter(|target| is_image_path(&target.path))
                    .collect();
                if images.is_empty() {
                    continue;
                }
                let params = serde_json::json!({
                    "repo_id": repo_id,
                    "files": images,
                    "reason": reason,
                });
                let summary = format!("Face-tag {} image(s)", images.len());
                if let Err(e) = self
                    .tasks
                    .submit_system(Origin::Schedule, JobKey::FaceTagScan, params, summary)
                    .await
                {
                    tracing::debug!("could not schedule face tags for {repo_id}: {e}");
                }
            }
        }
        submitted
    }
}

/// Whether the path has an extension the image worker can decode.
fn is_image_path(path: &str) -> bool {
    let ext = std::path::Path::new(path)
        .extension()
        .and_then(|ext| ext.to_str())
        .map(str::to_ascii_lowercase)
        .unwrap_or_default();
    crate::thumbnail_util::is_thumbnail_image_ext(&ext)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_page_below_the_threshold_is_scan_like() {
        assert!(pdf_page_is_scan_like(""));
        assert!(pdf_page_is_scan_like(" \n\t "));
        assert!(pdf_page_is_scan_like("page 1"));
    }

    #[test]
    fn a_text_page_is_not_scan_like() {
        assert!(!pdf_page_is_scan_like(&"word ".repeat(10)));
    }

    #[test]
    fn ocr_pages_replace_candidates_and_keep_empty_fallbacks() {
        let pages = vec!["typed".to_string(), "".to_string(), "caption".to_string()];
        let merged = merge_pdf_pages(pages, &[2, 3], "scanned\u{1e}");
        let extract::Extracted::Text(text) = merged else {
            panic!("the text layer still has a word");
        };
        let typed = text.find("typed").expect("the text page survives");
        let scanned = text.find("scanned").expect("the OCR page is merged");
        let caption = text
            .find("caption")
            .expect("the empty OCR field falls back");
        assert!(typed < scanned && scanned < caption, "{text:?}");
    }

    #[test]
    fn sandbox_off_stays_retryable_even_when_ocr_is_on() {
        assert_eq!(
            unsupported_status(extract::reason::SANDBOX_OFF, true),
            crate::indexer::DocStatus::Failed
        );
    }

    #[test]
    fn an_unsupported_ocr_input_is_terminal_no_text() {
        assert_eq!(
            unsupported_status(extract::reason::NO_TEXT, true),
            crate::indexer::DocStatus::NoText
        );
    }

    #[test]
    fn an_unsupported_input_without_ocr_is_skipped() {
        assert_eq!(
            unsupported_status(extract::reason::NO_TEXT, false),
            crate::indexer::DocStatus::Skipped
        );
    }

    #[test]
    fn an_empty_text_layer_with_no_ocr_text_is_unsupported() {
        let merged = merge_pdf_pages(vec![String::new()], &[1], "");
        assert!(matches!(merged, extract::Extracted::Unsupported(_)));
    }
}
