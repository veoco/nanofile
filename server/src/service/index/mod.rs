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

use crate::indexer::extract::{self, Plan};
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
}

impl IndexService {
    pub fn new(
        repos: Arc<Repositories>,
        block_store: DynBlockStorage,
        indexer: TextIndexer,
    ) -> Self {
        Self {
            repos,
            block_store,
            indexer,
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
                        state.is_manual() || !state.is_current(fs_id, now)
                    }
                    Some(state) => !state.is_current(fs_id, now),
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

        // A document above the cap is skipped unread: a container cannot be indexed
        // from a prefix.
        let size = file_data.size.max(0) as u64;
        let plan = extract::plan(filename);
        let budget = match extract::budget(plan, size) {
            Ok(budget) => budget,
            Err(reason) => {
                tracing::debug!("indexing {path}: not indexable: {reason}");
                self.indexer
                    .mark_file_async(repo_id, path, filename, DocMeta::skipped(fs_id.as_str()))
                    .await?;
                return Ok(OneOutcome::Skipped);
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

        // A document is read whole, so a short read is a transient failure.
        if matches!(plan, Plan::Document(_)) && (data.len() as u64) < size {
            tracing::warn!("indexing {path}: read {} of {size} bytes", data.len());
            self.indexer
                .mark_file_async(repo_id, path, filename, DocMeta::failed(fs_id.as_str()))
                .await?;
            return Ok(OneOutcome::Failed);
        }

        let extracted = if matches!(plan, extract::Plan::Document(_)) {
            // A document is parsed by a confined child process, never here: see
            // `sandbox::worker`. Spawning and waiting is synchronous and can take
            // seconds, so it runs on the blocking pool under a gate permit that bounds
            // how many children are alive; the permit is released when this branch ends.
            let _permit = structured_gate()
                .clone()
                .acquire_owned()
                .await
                .map_err(|e| AppError::internal(format!("document gate closed: {e}")))?;
            match offload_extract(ctx, move || worker::extract(plan, data)).await? {
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
            extract::extract(plan, data)
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
                // The sandbox being off is the admin's choice, not a verdict on the
                // file: a `Skipped` state is "current" for this extractor version, so
                // the document would never be re-extracted when the switch is turned
                // back on. It is recorded as a retryable failure instead, which costs
                // nothing because `extract` returns before spawning a child.
                if reason == extract::reason::SANDBOX_OFF {
                    self.indexer
                        .mark_file_async(repo_id, path, filename, DocMeta::failed(fs_id.as_str()))
                        .await?;
                    Ok(OneOutcome::Failed)
                } else {
                    self.indexer
                        .mark_file_async(repo_id, path, filename, DocMeta::skipped(fs_id.as_str()))
                        .await?;
                    Ok(OneOutcome::Skipped)
                }
            }
        }
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
    enabled: bool,
}

impl IndexScheduler {
    pub fn new(tasks: Arc<TaskSystem>, enabled: bool) -> Self {
        Self { tasks, enabled }
    }

    /// Whether scheduling can do anything; callers skip building the file list
    /// when it cannot.
    pub fn is_enabled(&self) -> bool {
        self.enabled
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
        if !self.enabled || targets.is_empty() {
            return 0;
        }
        let mut submitted = 0;
        for chunk in targets.chunks(BATCH_FILES) {
            let files: Vec<&IndexTarget> = chunk.iter().collect();
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
        submitted
    }
}
