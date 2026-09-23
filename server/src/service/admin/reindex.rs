use std::sync::Arc;

use futures::StreamExt;

use crate::indexer::{TextIndexer, collect_file_paths_under};
use crate::repository::Repositories;
use crate::tasks::JobFailure;
use crate::tasks::context::JobContext;
use base::error::AppError;
use infra::common::EMPTY_SHA1;
use infra::storage::DynBlockStorage;

/// How many files a pass reindexes at once, before the running budget trims it.
const REINDEX_CONCURRENCY: usize = 8;

/// Returned when a pass stopped at a checkpoint rather than finishing.
///
/// The job body maps it back onto a cancellation, which is a terminal state
/// distinct from a failure — the same arrangement GC uses.
pub const ABORTED: &str = "reindex stopped at a checkpoint";

/// Service for index/reindex administration operations.
pub struct AdminService {
    repos: Arc<Repositories>,
}

impl AdminService {
    pub fn new(repos: Arc<Repositories>) -> Self {
        Self { repos }
    }

    /// Verify that the authenticated user can access a repository.
    pub async fn check_repo_access(
        &self,
        repo_id: &str,
        user_id: i32,
    ) -> Result<infra::entity::repo::Model, AppError> {
        let repo_model = self
            .repos
            .repo
            .find_by_id(repo_id)
            .await?
            .ok_or_else(|| AppError::NotFound("repo not found".into()))?;
        if repo_model.owner_id != user_id {
            let is_member = self
                .repos
                .member
                .find_by_repo_and_user(repo_id, user_id)
                .await?
                .is_some();
            if !is_member {
                return Err(AppError::Forbidden);
            }
        }
        Ok(repo_model)
    }

    /// Verify that the user is the repo owner or a server admin.
    /// Stricter than `check_repo_access` — used for heavy operations
    /// (full-text reindex) that must not be triggerable by read-only members.
    pub async fn check_repo_admin(
        &self,
        repo_id: &str,
        user_id: i32,
    ) -> Result<infra::entity::repo::Model, AppError> {
        let repo_model = self
            .repos
            .repo
            .find_by_id(repo_id)
            .await?
            .ok_or_else(|| AppError::NotFound("repo not found".into()))?;
        if repo_model.owner_id != user_id {
            let user = self
                .repos
                .user
                .find_by_id(user_id)
                .await?
                .ok_or(AppError::Forbidden)?;
            if !user.is_admin {
                return Err(AppError::Forbidden);
            }
        }
        Ok(repo_model)
    }

    /// Index a single file with custom extracted text.
    pub async fn index_file_text(
        &self,
        indexer: &TextIndexer,
        repo_id: &str,
        path: &str,
        text: &str,
    ) -> Result<(), AppError> {
        let fullpath = if path.starts_with('/') {
            path.to_string()
        } else {
            format!("/{path}")
        };
        let filename = fullpath
            .rsplit_once('/')
            .map(|(_, name)| name)
            .unwrap_or(&fullpath);

        indexer
            .index_file_async(repo_id, &fullpath, filename, text)
            .await
            .map_err(|e| AppError::Internal(format!("index failed: {e}")))
    }

    /// Rebuild the full-text search index for all files in a repository.
    ///
    /// Files are reindexed with bounded concurrency (whole-file reads +
    /// Tantivy writes are heavy); `on_progress` is called after each file with
    /// `(done_count, total)` so a background task can report progress.
    ///
    /// `ctx` is the running job's context. Every file checks in before it is
    /// read, so on a busy server the pass stops at a file boundary and resumes
    /// when the server is quiet — the in-flight files park and the stream stops
    /// pulling new ones, so the concurrency drains to zero rather than merely
    /// slowing down. `None` is the uninterruptible call used outside a job.
    pub async fn reindex(
        &self,
        indexer: &TextIndexer,
        repo_id: &str,
        block_store: &DynBlockStorage,
        ctx: Option<&JobContext>,
        mut on_progress: impl FnMut(u64, u64) + Send + 'static,
    ) -> Result<(u64, u64), AppError> {
        let repo_model = self
            .repos
            .repo
            .find_by_id(repo_id)
            .await?
            .ok_or_else(|| AppError::NotFound("repo not found".into()))?;

        let head_commit_id = repo_model
            .head_commit_id
            .ok_or_else(|| AppError::NotFound("repo has no commits".into()))?;

        let head = self
            .repos
            .commit
            .find_by_id(&head_commit_id)
            .await?
            .ok_or_else(|| AppError::NotFound("head commit not found".into()))?;

        if head.root_id == EMPTY_SHA1 {
            return Ok((0, 0));
        }

        let file_paths = collect_file_paths_under(&self.repos, repo_id, &head.root_id, "").await?;
        let total = file_paths.len() as u64;

        // The budget trims the width when the pass starts on an already busy
        // server. It is not what holds a *running* pass back — the checkpoint
        // is, because a parked file stops the stream pulling the next one.
        let width = (REINDEX_CONCURRENCY as f32 * ctx.map_or(1.0, JobContext::budget)) as usize;

        let mut stream = futures::stream::iter(file_paths)
            .map(|fullpath| {
                let indexer = indexer.clone();
                let block_store = block_store.clone();
                let rid = repo_id.to_string();
                async move {
                    // Checked in before the read, not after: the point is not to
                    // start work the server cannot afford right now.
                    checkpoint(ctx).await?;
                    Ok::<bool, AppError>(
                        indexer
                            .reindex_file(&rid, &fullpath, &block_store)
                            .await
                            // A file that cannot be read is not a reason to
                            // abandon a rebuild; it counts as skipped, as it
                            // always has.
                            .unwrap_or(false),
                    )
                }
            })
            .buffer_unordered(width.max(1));

        let mut indexed = 0u64;
        let mut skipped = 0u64;
        let mut done = 0u64;
        while let Some(result) = stream.next().await {
            if result? {
                indexed += 1;
            } else {
                skipped += 1;
            }
            done += 1;
            on_progress(done, total);
        }

        Ok((indexed, skipped))
    }
}

/// Ask the run to stop, if there is a run to ask.
async fn checkpoint(ctx: Option<&JobContext>) -> Result<(), AppError> {
    let Some(ctx) = ctx else {
        return Ok(());
    };
    ctx.checkpoint().await.map_err(|failure| match failure {
        // Mapped back onto a cancellation by the job body, which is the only
        // place that knows which of the two terminal states applies.
        JobFailure::Cancelled | JobFailure::TimedOut => AppError::OperationFailed(ABORTED.into()),
        JobFailure::App(e) => e,
    })
}
