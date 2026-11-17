use crate::fs::core::tree::read_fs_file_data_batch;
use crate::fs::core::upload_activity;
use crate::repository::Repositories;
use crate::repository::gc_state::LAST_AUDIT_AT;
use crate::tasks::run::JobFailure;
use base::common::{FsDirData, S_IFDIR, SEAF_METADATA_TYPE_DIR};
use base::error::AppError;
use infra::entity::{commit, repo};
use infra::storage::DynBlockStorage;
use std::time::Duration;

const SECS_PER_DAY: i64 = 86_400;

/// Returned when the pass stopped at a checkpoint on request; mapped back to a cancellation.
pub const ABORTED: &str = "gc stopped at a checkpoint";

/// Tunables for one garbage-collection pass.
#[derive(Debug, Clone, Copy)]
pub struct GcPolicy {
    /// Blocks written less than this many seconds ago are never deleted, even
    /// when the snapshot does not mention them. `0` disables the window.
    pub min_block_age_secs: u64,
    /// Re-read each repository's head right before deleting its blocks and skip
    /// that repository when the head moved after the snapshot was taken.
    pub verify_head: bool,
    /// Skip a repository whose write activity is younger than this. A sync
    /// upload spans several requests, so this lease is what keeps a pass from
    /// pruning FS objects between the commit upload and the branch update.
    /// `None` disables the check (reachability tests use that).
    pub upload_activity_ttl: Option<Duration>,
    /// How the pass finds the blocks it deletes.
    pub mode: GcMode,
    /// Blocks an upload announced and never committed are kept at least this long.
    /// `0` disables the sweep, leaving them to an audit.
    pub pending_ttl_secs: u64,
    /// In [`GcMode::Incremental`], how often a full audit replaces the incremental
    /// plan. `0` disables the automatic audit.
    pub audit_interval_secs: u64,
    /// Compute the deletions and report them without changing the database or the
    /// store. The same guards run, so the returned count is what a real pass would
    /// remove right now.
    pub dry_run: bool,
}

/// How a pass finds the blocks it deletes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum GcMode {
    /// Enumerate every block of each repository and delete the ones no retained
    /// commit references. One listing per repository, and the only mode that finds
    /// what a crash left behind (a block written before its commit arrived).
    #[default]
    Audit,
    /// Delete only the blocks this pass itself orphaned by pruning history: no
    /// enumeration, just the pruned file objects read from the database. It cannot
    /// see a crashed upload, which is what an occasional `nanofile gc --mode audit`
    /// is for.
    Incremental,
}

/// What one pass did, or — when it was a dry run — what it would have done.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct GcReport {
    /// fs_object rows pruned from the database.
    pub pruned_objects: u64,
    /// Blocks deleted (a dry run reports what it would delete).
    pub deleted_blocks: u64,
    /// Block prefixes of repositories that no longer exist.
    pub removed_repos: u64,
    /// Blocks kept because they are younger than the grace window.
    pub skipped_fresh: u64,
    /// Blocks removed because an upload announced them and never committed them.
    pub expired_pending: u64,
    /// Whether this pass planned with [`GcMode::Audit`] (in incremental mode, the
    /// periodic audit turns one pass into an audit).
    pub audited: bool,
}

impl GcReport {
    /// The same total [`GcManager::garbage_collect`] returns.
    pub fn total(&self) -> u64 {
        self.pruned_objects + self.deleted_blocks + self.removed_repos + self.expired_pending
    }

    fn merge(&mut self, other: GcReport) {
        self.pruned_objects += other.pruned_objects;
        self.deleted_blocks += other.deleted_blocks;
        self.removed_repos += other.removed_repos;
        self.skipped_fresh += other.skipped_fresh;
        self.expired_pending += other.expired_pending;
        self.audited |= other.audited;
    }
}

/// One repository's deletions, computed before anything is removed.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
struct RepoPlan {
    repo_id: String,
    /// fs_object rows to prune from the database.
    fs_object_rows: Vec<i64>,
    /// commit rows to prune from the database.
    commit_rows: Vec<i64>,
    /// Blocks this pass may delete.
    blocks: Vec<PlannedBlock>,
}

/// A block a pass intends to delete, with the age the enumeration reported.
#[derive(Debug, Clone, PartialEq, Eq)]
struct PlannedBlock {
    id: String,
    /// `None` when the enumeration had no age for it (the filesystem gets none
    /// from `readdir`); the delete path then asks the store for this one block.
    modified_secs: Option<i64>,
}

impl GcPolicy {
    /// The production policy: age grace window on, head re-checked, and recent
    /// uploads left alone.
    pub fn new(min_block_age_secs: u64) -> Self {
        Self {
            min_block_age_secs,
            verify_head: true,
            upload_activity_ttl: Some(upload_activity::UPLOAD_ACTIVITY_TTL),
            mode: GcMode::Audit,
            pending_ttl_secs: 0,
            audit_interval_secs: 0,
            dry_run: false,
        }
    }

    /// No grace window, no head re-check and no upload lease: what a reachability
    /// test wants (its blocks were just written, so the grace window would spare
    /// them).
    pub fn immediate() -> Self {
        Self {
            min_block_age_secs: 0,
            verify_head: false,
            upload_activity_ttl: None,
            mode: GcMode::Audit,
            pending_ttl_secs: 0,
            audit_interval_secs: 0,
            dry_run: false,
        }
    }
}

/// How many block deletions may run between checkpoints. Bounded by count rather
/// than time so the interval is predictable; one batch is a handful of unlinks on
/// the filesystem, or one object-store delete call.
const CHECKPOINT_EVERY_BLOCKS: usize = 256;

/// Ask the run to stop, if there is a run to ask.
async fn checkpoint(ctx: Option<&crate::tasks::context::JobContext>) -> Result<(), AppError> {
    let Some(ctx) = ctx else {
        return Ok(());
    };
    ctx.checkpoint().await.map_err(|failure| match failure {
        // The job body maps this back onto a cancellation: only it knows which
        // terminal state applies.
        JobFailure::Cancelled | JobFailure::TimedOut => AppError::OperationFailed(ABORTED.into()),
        JobFailure::App(e) => e,
    })
}

pub struct GcManager;

impl GcManager {
    /// Garbage-collect every repository: delete its unreachable blocks and remove
    /// block directories whose repository no longer exists.
    ///
    /// With per-repository block layout there is no cross-repository sharing, so a
    /// block is deletable once unreachable from every retained commit of its own
    /// repository (history_limit / history_ttl_days plus the current head).
    ///
    /// Reachability alone does not authorize a delete: an upload writes blocks
    /// *before* the commit that references them, so a block can be live while
    /// absent from the snapshot — [`GcPolicy`] carries the two guards for that window.
    pub async fn garbage_collect(
        repos: &Repositories,
        block_store: &DynBlockStorage,
        policy: GcPolicy,
    ) -> Result<u64, AppError> {
        Self::garbage_collect_with(repos, block_store, policy, None).await
    }

    /// The same pass, but interruptible: `ctx` is the running job's context, and
    /// the pass checks in at every repository boundary and after each batch of
    /// block deletions, which is what lets it step aside for load.
    pub async fn garbage_collect_with(
        repos: &Repositories,
        block_store: &DynBlockStorage,
        policy: GcPolicy,
        ctx: Option<&crate::tasks::context::JobContext>,
    ) -> Result<u64, AppError> {
        Ok(
            Self::garbage_collect_report(repos, block_store, policy, ctx)
                .await?
                .total(),
        )
    }

    /// The full result of a pass, for a caller that wants the breakdown (the
    /// `nanofile gc` command prints it).
    pub async fn garbage_collect_report(
        repos: &Repositories,
        block_store: &DynBlockStorage,
        policy: GcPolicy,
        ctx: Option<&crate::tasks::context::JobContext>,
    ) -> Result<GcReport, AppError> {
        let now = chrono::Utc::now().timestamp();

        // An incremental schedule still needs a full audit eventually: it is the
        // only planner that finds what a crash left behind. When one is due, this
        // pass *is* the audit; the timestamp is written only after it succeeds.
        let incremental_schedule = policy.mode == GcMode::Incremental;
        let audit_due = match policy.mode {
            GcMode::Audit => true,
            GcMode::Incremental => {
                policy.audit_interval_secs > 0
                    && match repos.gc_state.get_i64(LAST_AUDIT_AT).await? {
                        Some(last) => now.saturating_sub(last) >= policy.audit_interval_secs as i64,
                        None => true,
                    }
            }
        };
        let mut policy = policy;
        if audit_due {
            policy.mode = GcMode::Audit;
        }
        let mut report = GcReport {
            audited: audit_due,
            ..GcReport::default()
        };

        // Blocks an upload announced and never committed, past their window. This
        // runs before planning so the per-repository plans never see them, and it is
        // the only cleanup for them when the schedule is incremental.
        if policy.pending_ttl_secs > 0 {
            report.expired_pending +=
                Self::sweep_expired_pending(repos, block_store, &policy, now, ctx).await?;
        }

        let all_repos = repos.repo.find_all().await?;
        for repo_model in &all_repos {
            // A repository boundary is the natural place to wait: the previous
            // repository is done, so a later run can simply continue from here.
            checkpoint(ctx).await?;

            // A sync upload spans several requests. Any write in the last lease
            // window means this library may be mid-upload, so leave the whole
            // repository alone for this pass.
            if let Some(ttl) = policy.upload_activity_ttl
                && upload_activity::is_active(&repo_model.id, ttl)
            {
                tracing::debug!(
                    repo_id = %repo_model.id,
                    "skipping GC: recent upload activity"
                );
                continue;
            }

            // Plan first, delete second: the plan is a read-only description of what
            // this pass would remove, which is also exactly what a dry run reports.
            let Some(plan) = Self::plan_repo(repos, block_store, repo_model, policy, now).await?
            else {
                continue;
            };
            report.merge(
                Self::apply_repo(repos, block_store, repo_model, &plan, policy, now, ctx).await?,
            );
        }

        report.removed_repos = Self::sweep_orphan_repos(repos, block_store, policy, ctx).await?;

        if incremental_schedule && audit_due && !policy.dry_run {
            // A failure only means the next pass audits again: the safe direction.
            if let Err(e) = repos.gc_state.set_i64(LAST_AUDIT_AT, now).await {
                tracing::warn!("could not record the time of the last audit: {e}");
            }
        }

        Ok(report)
    }

    /// Remove blocks an upload announced and never committed, once the pending
    /// window has passed.
    ///
    /// The row is claimed first, so a commit that cleared it in the meantime wins
    /// and its block is left alone. A block that is referenced anyway (a row whose
    /// commit-side delete failed, or a deduplicated write this upload did not really
    /// own) is kept and its row is already gone — the safe direction, at the cost of
    /// an orphan an audit will find.
    async fn sweep_expired_pending(
        repos: &Repositories,
        block_store: &DynBlockStorage,
        policy: &GcPolicy,
        now: i64,
        ctx: Option<&crate::tasks::context::JobContext>,
    ) -> Result<u64, AppError> {
        /// Bound one pass so a long-neglected deployment cannot stall it.
        const LIMIT: usize = 10_000;

        let cutoff = now.saturating_sub(policy.pending_ttl_secs as i64);
        let expired = if policy.dry_run {
            repos.pending_block.expired_before(cutoff, LIMIT).await?
        } else {
            repos.pending_block.claim_expired(cutoff, LIMIT).await?
        };
        if expired.is_empty() {
            return Ok(0);
        }

        let mut by_repo: std::collections::BTreeMap<String, Vec<String>> =
            std::collections::BTreeMap::new();
        for (repo_id, block_id) in expired {
            by_repo.entry(repo_id).or_default().push(block_id);
        }

        let mut removed = 0u64;
        for (repo_id, ids) in by_repo {
            checkpoint(ctx).await?;
            let mut deletable = Vec::with_capacity(ids.len());
            for id in ids {
                match repos.fs_object.references_block(&repo_id, &id).await {
                    Ok(true) => {
                        tracing::debug!(
                            repo_id,
                            block_id = %id,
                            "keeping an expired pending block: a file references it"
                        );
                    }
                    Ok(false) => deletable.push(id),
                    Err(e) => {
                        // Unknown: keep the block rather than risk deleting data.
                        tracing::warn!(
                            repo_id,
                            block_id = %id,
                            "could not check references for an expired pending block: {e}"
                        );
                    }
                }
            }
            for chunk in deletable.chunks(CHECKPOINT_EVERY_BLOCKS) {
                if policy.dry_run {
                    removed += chunk.len() as u64;
                } else {
                    removed += block_store.remove_blocks(&repo_id, chunk).await?;
                }
            }
        }
        Ok(removed)
    }

    /// Whether the repository's head differs from the snapshot this pass read. A
    /// repository that disappeared entirely counts as moved: its blocks are no
    /// longer this pass's to judge (the orphan-directory sweep handles them).
    async fn repo_head_moved(
        repos: &Repositories,
        snapshot: &repo::Model,
    ) -> Result<bool, AppError> {
        Ok(match repos.repo.find_by_id(&snapshot.id).await? {
            Some(current) => current.head_commit_id != snapshot.head_commit_id,
            None => true,
        })
    }

    /// Remove the block prefix of every repository that has neither a `repos` row
    /// nor a trash entry.
    ///
    /// Deleting a repository removes its rows (cascading to commits and fs_objects)
    /// but leaves its block prefix, which would otherwise stay in the store
    /// forever. A repository whose id is still in the trash table is kept:
    /// restoring a deleted library reuses the same repository id.
    ///
    /// The sweep is also the only cleanup for what a crash left behind (a block
    /// written before its commit arrived), which is why it stays available even in
    /// [`GcMode::Incremental`].
    async fn sweep_orphan_repos(
        repos: &Repositories,
        block_store: &DynBlockStorage,
        policy: GcPolicy,
        ctx: Option<&crate::tasks::context::JobContext>,
    ) -> Result<u64, AppError> {
        let mut removed = 0u64;
        for repo_id in block_store.repo_ids().await? {
            checkpoint(ctx).await?;
            if repos.repo.find_by_id(&repo_id).await?.is_some() {
                continue;
            }
            if repos.deleted_repo.find_by_id(&repo_id).await?.is_some() {
                continue;
            }
            if policy.dry_run {
                tracing::info!(
                    repo_id = %repo_id,
                    "would remove the block prefix of a repository that no longer exists"
                );
                removed += 1;
                continue;
            }
            tracing::info!(
                repo_id = %repo_id,
                "removing the block prefix of a repository that no longer exists"
            );
            match block_store.remove_repo(&repo_id).await {
                Ok(true) => removed += 1,
                Ok(false) => {}
                Err(e) => tracing::warn!(
                    repo_id = %repo_id,
                    "failed to remove the orphan block prefix: {e}"
                ),
            }
        }
        Ok(removed)
    }

    /// What this pass would delete from one repository. Read-only: it takes the
    /// database snapshot, walks the retained history, and (in [`GcMode::Audit`])
    /// enumerates the store. Nothing is removed until [`Self::apply_repo`].
    async fn plan_repo(
        repos: &Repositories,
        block_store: &DynBlockStorage,
        repo_model: &repo::Model,
        policy: GcPolicy,
        now: i64,
    ) -> Result<Option<RepoPlan>, AppError> {
        // GC needs every commit, so it passes `None` to fetch all rows.
        let commits = repos
            .commit
            .find_by_repo_id_ordered_by_ctime_desc(&repo_model.id, None)
            .await?;
        if commits.is_empty() {
            return Ok(None);
        }

        // Snapshot the FS rows this pass is allowed to prune. A row inserted after
        // this point may belong to an upload that started during the pass, so it
        // must never be a candidate here.
        let prunes_history = repo_model.history_limit != 0 || repo_model.history_ttl_days != 0;
        let fs_snapshot = if prunes_history {
            Some(
                repos
                    .fs_object
                    .find_ids_and_fs_ids_by_repo_id(&repo_model.id)
                    .await?,
            )
        } else {
            None
        };

        let keep = Self::compute_keep_commits(
            &commits,
            repo_model.head_commit_id.as_deref(),
            repo_model.history_limit,
            repo_model.history_ttl_days,
            now,
        );

        // Block ids referenced by this repository's retained history; rebuilt per
        // repository because there is no cross-repository sharing. `reachable` is
        // the set of fs ids those commits can see.
        let mut alive_blocks: std::collections::HashSet<String> = std::collections::HashSet::new();
        let reachable = Self::collect_repo_alive_blocks(
            repos,
            &repo_model.id,
            &commits,
            &keep,
            &mut alive_blocks,
        )
        .await?;

        let mut plan = RepoPlan {
            repo_id: repo_model.id.clone(),
            ..RepoPlan::default()
        };

        // Only a pass that actually prunes commits deletes rows. A repository whose
        // retention keeps every commit leaves even the rows no commit references
        // (leftovers from an interrupted upload) to the block sweep, as before.
        // The incremental planner derives its candidates from exactly these rows:
        // deleting blocks while their rows stay behind would leave objects the
        // database still points at.
        let prunes_commits = keep.len() != commits.len();
        let mut pruned_fs_ids: Vec<String> = Vec::new();
        if let Some(snapshot) = fs_snapshot.as_ref()
            && prunes_commits
        {
            plan.fs_object_rows = snapshot
                .iter()
                .filter(|(_, fs_id)| !reachable.contains(fs_id))
                .map(|(id, _)| *id)
                .collect();
            plan.commit_rows = commits
                .iter()
                .filter(|c| !keep.contains(&c.id))
                .map(|c| c.id)
                .collect();
            pruned_fs_ids = snapshot
                .iter()
                .filter(|(_, fs_id)| !reachable.contains(fs_id))
                .map(|(_, fs_id)| hex::encode(fs_id))
                .collect();
        }

        plan.blocks = match policy.mode {
            GcMode::Audit => {
                // Stream the disk blocks so the full list is never materialised;
                // only the orphan subset (usually tiny) is collected. A `'static`
                // callback cannot borrow, so the live set and the collector are
                // shared through Arc.
                let alive_blocks = std::sync::Arc::new(alive_blocks);
                let orphan_blocks =
                    std::sync::Arc::new(std::sync::Mutex::new(Vec::<PlannedBlock>::new()));
                let (alive_ref, orphan_ref) = (alive_blocks.clone(), orphan_blocks.clone());
                block_store
                    .for_each_block_with_mtime(
                        &repo_model.id,
                        Box::new(move |id, modified_secs| {
                            if !alive_ref.contains(id) {
                                orphan_ref.lock().unwrap().push(PlannedBlock {
                                    id: id.to_string(),
                                    modified_secs,
                                });
                            }
                        }),
                    )
                    .await?;
                std::mem::take(&mut *orphan_blocks.lock().unwrap())
            }
            GcMode::Incremental => {
                Self::blocks_of_file_objects(repos, &repo_model.id, &pruned_fs_ids, &alive_blocks)
                    .await?
            }
        };

        // A block an upload announced recently is not garbage even when nothing
        // references it yet: the commit may still be in flight (the sync case), or
        // the upload may be resumable and paused between chunks. The ledger answers
        // this locally, so neither planner needs a per-block request.
        if policy.pending_ttl_secs > 0 && !plan.blocks.is_empty() {
            let cutoff = now.saturating_sub(policy.pending_ttl_secs as i64);
            let ids: Vec<String> = plan.blocks.iter().map(|block| block.id.clone()).collect();
            let fresh = repos
                .pending_block
                .written_after(&repo_model.id, &ids, cutoff)
                .await?;
            if !fresh.is_empty() {
                plan.blocks.retain(|block| !fresh.contains(&block.id));
            }
        }

        Ok(Some(plan))
    }

    /// Apply one repository's plan, re-running the two guards that make deleting
    /// blocks safe under a concurrent upload. A dry run performs the same reads and
    /// returns the same count without writing or deleting.
    async fn apply_repo(
        repos: &Repositories,
        block_store: &DynBlockStorage,
        snapshot: &repo::Model,
        plan: &RepoPlan,
        policy: GcPolicy,
        now: i64,
        ctx: Option<&crate::tasks::context::JobContext>,
    ) -> Result<GcReport, AppError> {
        // An upload may have started, or a commit may have landed, after the
        // snapshot this plan works from. Re-check right before the irreversible
        // deletes and leave the repository alone when either moved.
        if policy.verify_head && Self::repo_head_moved(repos, snapshot).await? {
            tracing::debug!(
                repo_id = %snapshot.id,
                planned_blocks = plan.blocks.len(),
                "skipping GC: the repository head moved during this pass"
            );
            return Ok(GcReport::default());
        }

        let mut report = GcReport::default();
        if !plan.fs_object_rows.is_empty() {
            if !policy.dry_run {
                repos
                    .fs_object
                    .delete_many_by_ids(plan.fs_object_rows.clone())
                    .await?;
            }
            report.pruned_objects += plan.fs_object_rows.len() as u64;
        }
        if !plan.commit_rows.is_empty() && !policy.dry_run {
            repos
                .commit
                .delete_many_by_ids(plan.commit_rows.clone())
                .await?;
        }

        if let Some(ttl) = policy.upload_activity_ttl
            && upload_activity::is_active(&snapshot.id, ttl)
        {
            tracing::debug!(
                repo_id = %snapshot.id,
                planned_blocks = plan.blocks.len(),
                "skipping block deletion: recent upload activity"
            );
            return Ok(report);
        }
        if policy.verify_head && Self::repo_head_moved(repos, snapshot).await? {
            tracing::warn!(
                repo_id = %snapshot.id,
                planned_blocks = plan.blocks.len(),
                "skipping block deletion: the repository head moved during this pass"
            );
            return Ok(report);
        }

        if !policy.dry_run {
            // Every delete below invalidates presence entries; a backend that
            // caches them gets the chance to drop the whole map once instead.
            block_store.invalidate_exists_cache();
        }

        for chunk in plan.blocks.chunks(CHECKPOINT_EVERY_BLOCKS) {
            // Many orphans can make a long tail, so check in periodically rather
            // than only between repositories.
            checkpoint(ctx).await?;
            let deletable = Self::deletable_blocks(
                chunk,
                block_store,
                &plan.repo_id,
                policy,
                now,
                &mut report.skipped_fresh,
            )
            .await?;
            if deletable.is_empty() {
                continue;
            }
            if policy.dry_run {
                report.deleted_blocks += deletable.len() as u64;
            } else {
                report.deleted_blocks +=
                    block_store.remove_blocks(&plan.repo_id, &deletable).await?;
            }
        }
        if report.skipped_fresh > 0 {
            tracing::debug!(
                repo_id = %plan.repo_id,
                skipped_fresh = report.skipped_fresh,
                min_block_age_secs = policy.min_block_age_secs,
                "kept blocks younger than the GC grace window"
            );
        }

        Ok(report)
    }

    /// The subset of one batch that is old enough to delete.
    ///
    /// The age comes from the enumeration when the backend reported it (an object
    /// store's listing carries `LastModified`), and is fetched for that one block
    /// otherwise — which is what the filesystem does, because `readdir` has no
    /// `stat`. A plan from the incremental planner is already filtered against the
    /// pending ledger, so it needs no store round trip at all.
    async fn deletable_blocks(
        chunk: &[PlannedBlock],
        block_store: &DynBlockStorage,
        repo_id: &str,
        policy: GcPolicy,
        now: i64,
        skipped_fresh: &mut u64,
    ) -> Result<Vec<String>, AppError> {
        if policy.min_block_age_secs == 0 || policy.mode == GcMode::Incremental {
            return Ok(chunk.iter().map(|block| block.id.clone()).collect());
        }
        let mut out = Vec::with_capacity(chunk.len());
        for block in chunk {
            let modified = match block.modified_secs {
                Some(secs) => Some(secs),
                None => block_store.block_modified_secs(repo_id, &block.id).await?,
            };
            // A block written at or after the snapshot may belong to an upload
            // whose commit is not visible yet — leave it for a later pass.
            if modified.is_some_and(|m| now.saturating_sub(m) < policy.min_block_age_secs as i64) {
                *skipped_fresh += 1;
                continue;
            }
            out.push(block.id.clone());
        }
        Ok(out)
    }

    /// Blocks referenced by the given file objects, minus the live set.
    ///
    /// The pruned side is read from the database in chunks: a large prune can name
    /// hundreds of thousands of objects, and their JSON is needed only here, never
    /// for the reachability walk. `read_fs_file_data_batch` drops directory objects,
    /// so only file ids put blocks into the result.
    async fn blocks_of_file_objects(
        repos: &Repositories,
        repo_id: &str,
        fs_ids: &[String],
        alive: &std::collections::HashSet<String>,
    ) -> Result<Vec<PlannedBlock>, AppError> {
        let mut out = Vec::new();
        let mut seen = std::collections::HashSet::new();
        for chunk in fs_ids.chunks(500) {
            let files = read_fs_file_data_batch(repos, repo_id, chunk).await?;
            for file in files.values() {
                for block_id in &file.block_ids {
                    if alive.contains(block_id) || !seen.insert(block_id.clone()) {
                        continue;
                    }
                    out.push(PlannedBlock {
                        id: block_id.clone(),
                        // Not an enumeration. The incremental planner has already
                        // filtered the list against the pending ledger, so the delete
                        // path treats it as old without asking the store.
                        modified_secs: None,
                    });
                }
            }
        }
        Ok(out)
    }

    /// Compute the set of commit ids (by DB primary key) to retain.
    /// `history_limit > 0` keeps the newest N; `history_ttl_days > 0` keeps
    /// commits newer than the cutoff. With both at 0 the repo is unlimited, so
    /// **every** commit is retained — otherwise its blocks would be
    /// mis-identified as orphans.
    ///
    /// The current head is always retained, even when it falls outside both
    /// windows: `ctime` is client-supplied, so the head is not necessarily the
    /// newest row, and a TTL can expire while the library sits idle. Dropping it
    /// would delete the tree the library currently serves.
    fn compute_keep_commits(
        commits: &[commit::Model],
        head_commit_id: Option<&str>,
        history_limit: i32,
        history_ttl_days: i32,
        now: i64,
    ) -> std::collections::HashSet<i64> {
        let mut keep = std::collections::HashSet::new();
        if let Some(head_id) = head_commit_id
            && let Some(head) = commits.iter().find(|c| c.commit_id.as_str() == head_id)
        {
            keep.insert(head.id);
        }
        if history_limit > 0 {
            for c in commits.iter().take(history_limit as usize) {
                keep.insert(c.id);
            }
        }
        if history_ttl_days > 0 {
            let cutoff = now - i64::from(history_ttl_days) * SECS_PER_DAY;
            for c in commits {
                if c.ctime >= cutoff {
                    keep.insert(c.id);
                }
            }
        }
        if history_limit == 0 && history_ttl_days == 0 {
            for c in commits {
                keep.insert(c.id);
            }
        }
        keep
    }

    /// Collect the block ids referenced by any file reachable from the retained
    /// commits of one repo. The per-repository layout makes this exactly the set of
    /// blocks that repository may keep, i.e. the authorization-relevant live set.
    async fn collect_repo_alive_blocks(
        repos: &Repositories,
        repo_id: &str,
        commits: &[commit::Model],
        keep: &std::collections::HashSet<i64>,
        alive_blocks: &mut std::collections::HashSet<String>,
    ) -> Result<std::collections::HashSet<[u8; 20]>, AppError> {
        if commits.is_empty() {
            return Ok(std::collections::HashSet::new());
        }

        // Every fs_id reachable from the retained commits' roots. Returned so
        // `plan_repo` can reuse it instead of re-walking the same trees.
        let mut reachable = std::collections::HashSet::new();
        for c in commits {
            if keep.contains(&c.id) {
                Self::collect_fs_ids(repos, repo_id, &c.root_id, &mut reachable).await?;
            }
        }
        if reachable.is_empty() {
            return Ok(reachable);
        }

        // Batch-fetch the reachable *file* objects and record their block ids.
        let ids: Vec<String> = reachable.iter().map(hex::encode).collect();
        let files = read_fs_file_data_batch(repos, repo_id, &ids).await?;
        for file in files.values() {
            for block_id in &file.block_ids {
                alive_blocks.insert(block_id.clone());
            }
        }
        Ok(reachable)
    }

    /// Decode a 40-char hex SHA-1 (block id or fs id) to its raw 20 bytes, so sets
    /// need no per-element heap allocation. Callers only pass validated 40-hex ids,
    /// so this cannot fail; a corrupt id decodes to all-zeroes.
    fn decode_sha1_hex(hex_str: &str) -> [u8; 20] {
        let mut buf = [0u8; 20];
        let _ = hex::decode_to_slice(hex_str, &mut buf);
        buf
    }

    /// Breadth-first walk from `root_id`, adding every reachable fs_id to
    /// `collected`; each level's directory objects are fetched in one batched query.
    async fn collect_fs_ids(
        repos: &Repositories,
        repo_id: &str,
        root_id: &str,
        collected: &mut std::collections::HashSet<[u8; 20]>,
    ) -> Result<(), AppError> {
        if collected.contains(&Self::decode_sha1_hex(root_id)) {
            return Ok(());
        }
        collected.insert(Self::decode_sha1_hex(root_id));

        let mut frontier = vec![root_id.to_string()];
        let mut guard = crate::fs::core::traversal::TreeGuard::new();
        while !frontier.is_empty() {
            guard.enter_level()?;
            guard.visit(frontier.len())?;
            // `fetch_fs_object_map` chunks the IN list under SQLite's variable limit;
            // `find_by_repo_and_fs_ids` does not.
            let objs =
                crate::fs::core::tree::fetch_fs_object_map(repos, repo_id, &frontier).await?;
            let mut next = Vec::new();
            for obj in objs.values() {
                if obj.obj_type == SEAF_METADATA_TYPE_DIR as i8 {
                    let dir_data: FsDirData = serde_json::from_str(&obj.data)
                        .map_err(|e| AppError::internal(e.to_string()))?;
                    for entry in &dir_data.dirents {
                        let is_new = collected.insert(Self::decode_sha1_hex(&entry.id));
                        // Only directories have children; use the dirent's `mode` bit
                        // so file objects are never fetched just to be ruled out.
                        if is_new && entry.mode & S_IFDIR != 0 {
                            next.push(entry.id.clone());
                        }
                    }
                }
            }
            frontier = next;
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use infra::storage::BlockStorageBackend;

    const REPO: &str = "test-repo";
    use sea_orm::{ConnectionTrait, Database, DatabaseBackend, Statement};
    use std::sync::Arc;

    /// A throwaway filesystem-backed block store; the `TempDir` keeps it alive.
    fn temp_block_store() -> (tempfile::TempDir, DynBlockStorage) {
        let dir = tempfile::tempdir().unwrap();
        let store = infra::storage::new_block_store(dir.path());
        (dir, store)
    }

    /// In-memory SQLite with the minimal schema and a seeded repo for GC.
    async fn setup_gc_test_db(
        history_limit: i32,
        history_ttl_days: i32,
    ) -> sea_orm::DatabaseConnection {
        setup_gc_test_db_for(REPO, history_limit, history_ttl_days).await
    }

    /// The same setup with an explicit repo id, so activity-lease tests can use a
    /// unique key instead of sharing the process-global activity map.
    async fn setup_gc_test_db_for(
        repo_id: &str,
        history_limit: i32,
        history_ttl_days: i32,
    ) -> sea_orm::DatabaseConnection {
        let db = Database::connect("sqlite::memory:").await.unwrap();

        db.execute_raw(Statement::from_string(
            DatabaseBackend::Sqlite,
            "
            CREATE TABLE repos (
                id VARCHAR(36) PRIMARY KEY NOT NULL,
                name VARCHAR(255) NOT NULL DEFAULT '',
                description TEXT NOT NULL DEFAULT '',
                owner_id INTEGER NOT NULL DEFAULT 0,
                encrypted TINYINT NOT NULL DEFAULT 0,
                enc_version TINYINT NOT NULL DEFAULT 0,
                magic VARCHAR(255),
                random_key VARCHAR(255),
                salt VARCHAR(255) NOT NULL DEFAULT '',
                pwd_hash VARCHAR(255),
                pwd_hash_algo VARCHAR(255),
                pwd_hash_params VARCHAR(255),
                head_commit_id VARCHAR(40),
                permission VARCHAR(10) NOT NULL DEFAULT 'rw',
                created_at BIGINT NOT NULL DEFAULT 0,
                updated_at BIGINT NOT NULL DEFAULT 0,
                size BIGINT NOT NULL DEFAULT 0,
                repo_version INTEGER NOT NULL DEFAULT 1,
                history_limit INTEGER NOT NULL DEFAULT 0,
                history_ttl_days INTEGER NOT NULL DEFAULT 0,
                type VARCHAR(20) NOT NULL DEFAULT 'repo'
            );

            CREATE TABLE commits (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                repo_id VARCHAR(36) NOT NULL,
                commit_id VARCHAR(40) NOT NULL,
                root_id VARCHAR(40) NOT NULL,
                parent_id VARCHAR(40),
                second_parent_id VARCHAR(40),
                creator_name VARCHAR(255) NOT NULL,
                creator VARCHAR(40) NOT NULL DEFAULT '0000000000000000000000000000000000000000',
                description TEXT NOT NULL DEFAULT '',
                ctime BIGINT NOT NULL,
                version TINYINT NOT NULL DEFAULT 1,
                new_merge BOOLEAN,
                conflict BOOLEAN
            );

            CREATE TABLE fs_objects (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                repo_id VARCHAR(36) NOT NULL,
                fs_id VARCHAR(40) NOT NULL,
                obj_type TINYINT NOT NULL,
                data TEXT NOT NULL
            );

            CREATE TABLE deleted_repos (
                repo_id VARCHAR(36) PRIMARY KEY,
                repo_name VARCHAR(255) NOT NULL DEFAULT '',
                head_id VARCHAR(40),
                owner_id INTEGER NOT NULL,
                size BIGINT NOT NULL DEFAULT 0,
                del_time BIGINT NOT NULL
            );

            CREATE TABLE pending_blocks (
                repo_id VARCHAR(36) NOT NULL,
                block_id VARCHAR(40) NOT NULL,
                written_at BIGINT NOT NULL,
                PRIMARY KEY (repo_id, block_id)
            );
            CREATE INDEX idx_pending_blocks_written_at ON pending_blocks (written_at);

            CREATE TABLE gc_state (
                key TEXT PRIMARY KEY,
                value TEXT NOT NULL,
                updated_at BIGINT NOT NULL
            );
            ",
        ))
        .await
        .unwrap();

        db.execute_raw(Statement::from_string(
            DatabaseBackend::Sqlite,
            format!(
                "INSERT INTO repos (id, name, description, owner_id, encrypted, enc_version, salt, \
                 permission, created_at, updated_at, size, repo_version, history_limit, history_ttl_days) \
                 VALUES ('{repo_id}', 'test', '', 1, 0, 0, '', 'rw', 0, 0, 0, 1, {history_limit}, {history_ttl_days})"
            ),
        ))
        .await
        .unwrap();

        db
    }

    async fn insert_commit(
        db: &sea_orm::DatabaseConnection,
        commit_id: &str,
        root_id: &str,
        ctime: i64,
    ) {
        insert_commit_for(db, REPO, commit_id, root_id, ctime).await;
    }

    async fn insert_commit_for(
        db: &sea_orm::DatabaseConnection,
        repo_id: &str,
        commit_id: &str,
        root_id: &str,
        ctime: i64,
    ) {
        db.execute_raw(Statement::from_string(
            DatabaseBackend::Sqlite,
            format!(
                "INSERT INTO commits (repo_id, commit_id, root_id, creator_name, ctime, version) \
                 VALUES ('{repo_id}', '{commit_id}', '{root_id}', 'u1', {ctime}, 1)"
            ),
        ))
        .await
        .unwrap();
    }

    /// Insert an fs_object. `obj_type` 3 = directory, 1 = file.
    async fn insert_fs_object(
        db: &sea_orm::DatabaseConnection,
        fs_id: &str,
        obj_type: i8,
        data: &str,
    ) {
        insert_fs_object_for(db, REPO, fs_id, obj_type, data).await;
    }

    async fn insert_fs_object_for(
        db: &sea_orm::DatabaseConnection,
        repo_id: &str,
        fs_id: &str,
        obj_type: i8,
        data: &str,
    ) {
        db.execute_raw(Statement::from_string(
            DatabaseBackend::Sqlite,
            format!(
                "INSERT INTO fs_objects (repo_id, fs_id, obj_type, data) \
                 VALUES ('{repo_id}', '{fs_id}', {obj_type}, '{data}')"
            ),
        ))
        .await
        .unwrap();
    }

    async fn count_rows(db: &sea_orm::DatabaseConnection, table: &str) -> u64 {
        use sea_orm::TryGetable;
        let row = db
            .query_one_raw(Statement::from_string(
                DatabaseBackend::Sqlite,
                format!("SELECT COUNT(*) AS c FROM {table}"),
            ))
            .await
            .unwrap()
            .expect("count row");
        i64::try_get(&row, "", "c").unwrap() as u64
    }

    /// With `history_limit = 1` the newest commit is kept, older commits are
    /// dropped, and fs objects unreachable from any kept commit are deleted.
    #[tokio::test]
    async fn test_gc_prunes_old_history_and_orphaned_fs_objects() {
        let db = setup_gc_test_db(1, 0).await;
        let repos = crate::repository::Repositories::new_for_tests(Arc::new(db.clone()));
        let (_dir, store) = temp_block_store();

        //   c1 (a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1 → b1b1b1b1b1b1b1b1b1b1b1b1b1b1b1b1b1b1b1b1), c2 (a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2), c3 (a3a3a3a3a3a3a3a3a3a3a3a3a3a3a3a3a3a3a3a3 → b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2)
        insert_commit(&db, "c1", "a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1", 3000).await;
        insert_commit(&db, "c2", "a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2", 2000).await;
        insert_commit(&db, "c3", "a3a3a3a3a3a3a3a3a3a3a3a3a3a3a3a3a3a3a3a3", 1000).await;

        // a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1 and a3a3a3a3a3a3a3a3a3a3a3a3a3a3a3a3a3a3a3a3 are directories referencing b1b1b1b1b1b1b1b1b1b1b1b1b1b1b1b1b1b1b1b1 / b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2.
        insert_fs_object(&db, "a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1", 3, r#"{"dirents":[{"id":"b1b1b1b1b1b1b1b1b1b1b1b1b1b1b1b1b1b1b1b1","mode":33188,"modifier":"u1","mtime":1000,"name":"a.txt","size":10}],"type":3,"version":1}"#).await;
        insert_fs_object(
            &db,
            "b1b1b1b1b1b1b1b1b1b1b1b1b1b1b1b1b1b1b1b1",
            1,
            r#"{"block_ids":["aaaa"],"size":10,"type":1,"version":1}"#,
        )
        .await;
        insert_fs_object(&db, "a3a3a3a3a3a3a3a3a3a3a3a3a3a3a3a3a3a3a3a3", 3, r#"{"dirents":[{"id":"b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2","mode":33188,"modifier":"u1","mtime":1000,"name":"b.txt","size":20}],"type":3,"version":1}"#).await;
        insert_fs_object(
            &db,
            "b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2",
            1,
            r#"{"block_ids":["bbbb"],"size":20,"type":1,"version":1}"#,
        )
        .await;

        let removed = GcManager::garbage_collect(&repos, &store, GcPolicy::immediate())
            .await
            .expect("gc succeeds");
        assert_eq!(removed, 2);

        assert_eq!(count_rows(&db, "commits").await, 1);
        // a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1 + b1b1b1b1b1b1b1b1b1b1b1b1b1b1b1b1b1b1b1b1 remain; a3a3a3a3a3a3a3a3a3a3a3a3a3a3a3a3a3a3a3a3 + b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2 deleted.
        assert_eq!(count_rows(&db, "fs_objects").await, 2);
    }

    /// A recent upload lease makes a pass skip the repository entirely; once the
    /// lease is treated as expired, normal retention pruning resumes.
    #[tokio::test]
    async fn test_gc_skips_repo_with_recent_upload_activity() {
        let repo = "gc-activity-test";
        let db = setup_gc_test_db_for(repo, 1, 0).await;
        let repos = crate::repository::Repositories::new_for_tests(Arc::new(db.clone()));
        let (_dir, store) = temp_block_store();

        let old_root = "e1".repeat(20);
        let head_root = "e2".repeat(20);
        insert_commit_for(&db, repo, &"e3".repeat(20), &old_root, 1000).await;
        insert_commit_for(&db, repo, &"e4".repeat(20), &head_root, 2000).await;
        insert_fs_object_for(
            &db,
            repo,
            &old_root,
            3,
            r#"{"dirents":[],"type":3,"version":1}"#,
        )
        .await;
        insert_fs_object_for(
            &db,
            repo,
            &head_root,
            3,
            r#"{"dirents":[],"type":3,"version":1}"#,
        )
        .await;

        crate::fs::core::upload_activity::touch(repo);

        GcManager::garbage_collect(&repos, &store, GcPolicy::new(0))
            .await
            .expect("gc succeeds");
        assert_eq!(
            count_rows(&db, "commits").await,
            2,
            "activity must skip the repository"
        );

        // Treat the lease as expired: the same pass now prunes the older commit.
        let expired = GcPolicy {
            upload_activity_ttl: Some(Duration::ZERO),
            ..GcPolicy::new(0)
        };
        GcManager::garbage_collect(&repos, &store, expired)
            .await
            .expect("gc succeeds");
        assert_eq!(count_rows(&db, "commits").await, 1);
        assert_eq!(count_rows(&db, "fs_objects").await, 1);
        crate::fs::core::upload_activity::clear(repo);
    }

    /// Planning takes the FS-object snapshot; a row inserted after the pass
    /// started must survive even when it is unreachable.
    #[tokio::test]
    async fn test_repo_plan_snapshots_rows() {
        let repo = "gc-snapshot-test";
        let db = setup_gc_test_db_for(repo, 1, 0).await;
        let repos = crate::repository::Repositories::new_for_tests(Arc::new(db.clone()));
        let (_dir, store) = temp_block_store();

        let older_root = "f1".repeat(20);
        let old_root = "f2".repeat(20);
        let head_root = "f3".repeat(20);
        let late_root = "f4".repeat(20);
        insert_commit_for(&db, repo, &"f5".repeat(20), &older_root, 1000).await;
        insert_commit_for(&db, repo, &"f6".repeat(20), &old_root, 1500).await;
        insert_commit_for(&db, repo, &"f7".repeat(20), &head_root, 2000).await;
        for root in [&older_root, &old_root, &head_root] {
            insert_fs_object_for(&db, repo, root, 3, r#"{"dirents":[],"type":3,"version":1}"#)
                .await;
        }

        // The plan reads the rows that exist now; `late_root` is inserted after it,
        // so the plan cannot name it.
        let repo_model = repos.repo.find_by_id(repo).await.unwrap().unwrap();
        let policy = GcPolicy {
            upload_activity_ttl: None,
            verify_head: false,
            ..GcPolicy::new(0)
        };
        let plan = GcManager::plan_repo(&repos, &store, &repo_model, policy, 10_000)
            .await
            .expect("plan succeeds")
            .expect("repo has commits");

        insert_fs_object_for(
            &db,
            repo,
            &late_root,
            3,
            r#"{"dirents":[],"type":3,"version":1}"#,
        )
        .await;

        let report =
            GcManager::apply_repo(&repos, &store, &repo_model, &plan, policy, 10_000, None)
                .await
                .expect("apply succeeds");

        assert_eq!(
            report.pruned_objects, 2,
            "both snapshot FS rows are unreachable"
        );
        assert!(
            repos
                .fs_object
                .find_by_repo_and_fs_id(repo, &late_root)
                .await
                .unwrap()
                .is_some(),
            "a row inserted after the snapshot must survive"
        );
        // The retained head's root and the late row.
        assert_eq!(count_rows(&db, "fs_objects").await, 2);
        assert_eq!(count_rows(&db, "commits").await, 1);
    }

    /// Two commits, one file each, sharing one block. With `history_limit = 1` the
    /// older commit is pruned, so exactly one block becomes an orphan.
    async fn seed_pruned_history(
        db: &sea_orm::DatabaseConnection,
        store: &DynBlockStorage,
    ) -> (String, String) {
        let old_block = store.write_block(REPO, b"old-only").await.unwrap();
        let shared = store.write_block(REPO, b"shared").await.unwrap();

        let dir_old = "d1".repeat(20);
        let dir_head = "d2".repeat(20);
        let file_old = "e1".repeat(20);
        let file_head = "e2".repeat(20);
        insert_commit(db, "c1", &dir_old, 1000).await;
        insert_commit(db, "c2", &dir_head, 2000).await;
        insert_fs_object(
            db,
            &dir_old,
            3,
            &format!(
                r#"{{"dirents":[{{"id":"{file_old}","mode":33188,"modifier":"u1","mtime":1000,"name":"old.txt","size":18}}],"type":3,"version":1}}"#
            ),
        )
        .await;
        insert_fs_object(
            db,
            &dir_head,
            3,
            &format!(
                r#"{{"dirents":[{{"id":"{file_head}","mode":33188,"modifier":"u1","mtime":2000,"name":"head.txt","size":6}}],"type":3,"version":1}}"#
            ),
        )
        .await;
        insert_fs_object(
            db,
            &file_old,
            1,
            &format!(
                r#"{{"block_ids":["{old_block}","{shared}"],"size":18,"type":1,"version":1}}"#
            ),
        )
        .await;
        insert_fs_object(
            db,
            &file_head,
            1,
            &format!(r#"{{"block_ids":["{shared}"],"size":6,"type":1,"version":1}}"#),
        )
        .await;
        (old_block, shared)
    }

    fn planned_ids(plan: &RepoPlan) -> Vec<String> {
        let mut ids: Vec<String> = plan.blocks.iter().map(|block| block.id.clone()).collect();
        ids.sort();
        ids
    }

    /// Both planners must delete the same blocks: the audit by listing, the
    /// incremental by reading the pruned objects.
    #[tokio::test]
    async fn incremental_and_audit_plans_agree() {
        let db = setup_gc_test_db(1, 0).await;
        let repos = crate::repository::Repositories::new_for_tests(Arc::new(db.clone()));
        let (_dir, store) = temp_block_store();
        let (old_block, shared) = seed_pruned_history(&db, &store).await;

        let repo_model = repos.repo.find_by_id(REPO).await.unwrap().unwrap();
        let base = GcPolicy {
            upload_activity_ttl: None,
            verify_head: false,
            ..GcPolicy::new(0)
        };
        let audit = GcManager::plan_repo(
            &repos,
            &store,
            &repo_model,
            GcPolicy {
                mode: GcMode::Audit,
                ..base
            },
            10_000,
        )
        .await
        .unwrap()
        .expect("repo has commits");
        let incremental = GcManager::plan_repo(
            &repos,
            &store,
            &repo_model,
            GcPolicy {
                mode: GcMode::Incremental,
                ..base
            },
            10_000,
        )
        .await
        .unwrap()
        .expect("repo has commits");

        assert_eq!(
            planned_ids(&audit),
            planned_ids(&incremental),
            "both planners must agree on the orphan set"
        );
        assert_eq!(planned_ids(&incremental), vec![old_block.clone()]);
        assert_eq!(audit.fs_object_rows.len(), 2);
        assert_eq!(audit.fs_object_rows, incremental.fs_object_rows);

        let report = GcManager::apply_repo(
            &repos,
            &store,
            &repo_model,
            &incremental,
            GcPolicy {
                mode: GcMode::Incremental,
                ..base
            },
            10_000,
            None,
        )
        .await
        .unwrap();
        assert_eq!(report.pruned_objects, 2);
        assert_eq!(report.deleted_blocks, 1);
        assert!(!store.has_block(REPO, &old_block).await);
        assert!(store.has_block(REPO, &shared).await);
    }

    /// A dry run reports what a real pass would remove and changes nothing.
    #[tokio::test]
    async fn dry_run_reports_without_deleting() {
        let db = setup_gc_test_db(1, 0).await;
        let repos = crate::repository::Repositories::new_for_tests(Arc::new(db.clone()));
        let (_dir, store) = temp_block_store();
        let (old_block, shared) = seed_pruned_history(&db, &store).await;
        let policy = GcPolicy {
            upload_activity_ttl: None,
            verify_head: false,
            ..GcPolicy::new(0)
        };

        let dry = GcManager::garbage_collect_report(
            &repos,
            &store,
            GcPolicy {
                dry_run: true,
                ..policy
            },
            None,
        )
        .await
        .expect("dry run succeeds");

        assert_eq!(dry.pruned_objects, 2);
        assert_eq!(dry.deleted_blocks, 1);
        assert_eq!(dry.total(), 3);
        assert_eq!(count_rows(&db, "fs_objects").await, 4);
        assert_eq!(count_rows(&db, "commits").await, 2);
        assert!(store.has_block(REPO, &old_block).await);

        let real = GcManager::garbage_collect_report(&repos, &store, policy, None)
            .await
            .expect("gc succeeds");

        assert_eq!(real.total(), dry.total(), "a dry run must predict the pass");
        assert_eq!(count_rows(&db, "fs_objects").await, 2);
        assert_eq!(count_rows(&db, "commits").await, 1);
        assert!(!store.has_block(REPO, &old_block).await);
        assert!(store.has_block(REPO, &shared).await);
    }

    /// A block prefix with neither a `repos` row nor a trash entry is removed.
    #[tokio::test]
    async fn sweep_removes_prefixes_of_repositories_that_no_longer_exist() {
        let db = setup_gc_test_db(0, 0).await;
        let repos = crate::repository::Repositories::new_for_tests(Arc::new(db.clone()));
        let (_dir, store) = temp_block_store();
        let ghost = "ghost-repo";
        store.write_block(ghost, b"ghost bytes").await.unwrap();
        assert!(store.repo_ids().await.unwrap().contains(&ghost.to_string()));

        let report = GcManager::garbage_collect_report(
            &repos,
            &store,
            GcPolicy {
                upload_activity_ttl: None,
                verify_head: false,
                ..GcPolicy::new(0)
            },
            None,
        )
        .await
        .expect("gc succeeds");

        assert_eq!(report.removed_repos, 1);
        assert!(
            !store.repo_ids().await.unwrap().contains(&ghost.to_string()),
            "the orphan prefix must be gone"
        );
    }

    /// `(repo, block) → (bytes, modification time)` for the fake object store.
    type StoredBlocks =
        std::sync::Mutex<std::collections::HashMap<(String, String), (Vec<u8>, i64)>>;

    /// A backend shaped like an object store: its enumeration reports an age and its
    /// deletes are batched. Both are what the S3 backend will do, and both are easy to
    /// lose again — a regression here would put one HEAD and one DELETE per block back
    /// on the wire.
    #[derive(Debug, Default)]
    struct ObjectStoreLike {
        blocks: StoredBlocks,
        delete_calls: std::sync::atomic::AtomicUsize,
        repo_removals: std::sync::atomic::AtomicUsize,
    }

    impl ObjectStoreLike {
        fn put(&self, repo_id: &str, block_id: &str, bytes: &[u8], modified: i64) {
            self.blocks.lock().unwrap().insert(
                (repo_id.to_string(), block_id.to_string()),
                (bytes.to_vec(), modified),
            );
        }
    }

    #[async_trait::async_trait]
    impl infra::storage::BlockStorageBackend for ObjectStoreLike {
        async fn has_block(&self, repo_id: &str, block_id: &str) -> bool {
            self.blocks
                .lock()
                .unwrap()
                .contains_key(&(repo_id.to_string(), block_id.to_string()))
        }

        async fn read_block(
            &self,
            repo_id: &str,
            block_id: &str,
        ) -> Result<Vec<u8>, std::io::Error> {
            self.blocks
                .lock()
                .unwrap()
                .get(&(repo_id.to_string(), block_id.to_string()))
                .map(|(bytes, _)| bytes.clone())
                .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::NotFound, "no block"))
        }

        async fn write_block(&self, repo_id: &str, data: &[u8]) -> Result<String, std::io::Error> {
            let id = infra::crypto::fs_id::sha1_hex(data);
            self.put(repo_id, &id, data, 0);
            Ok(id)
        }

        async fn remove_block(&self, repo_id: &str, block_id: &str) -> Result<(), std::io::Error> {
            self.blocks
                .lock()
                .unwrap()
                .remove(&(repo_id.to_string(), block_id.to_string()));
            Ok(())
        }

        async fn block_size(&self, repo_id: &str, block_id: &str) -> Result<i64, std::io::Error> {
            let blocks = self.blocks.lock().unwrap();
            match blocks.get(&(repo_id.to_string(), block_id.to_string())) {
                Some((bytes, _)) => Ok(bytes.len() as i64),
                None => Err(std::io::Error::new(
                    std::io::ErrorKind::NotFound,
                    "no block",
                )),
            }
        }

        async fn list_blocks(&self, repo_id: &str) -> Result<Vec<String>, std::io::Error> {
            Ok(self
                .blocks
                .lock()
                .unwrap()
                .keys()
                .filter(|(repo, _)| repo == repo_id)
                .map(|(_, block)| block.clone())
                .collect())
        }

        async fn for_each_block_with_mtime(
            &self,
            repo_id: &str,
            mut f: Box<dyn for<'a> FnMut(&'a str, Option<i64>) + Send>,
        ) -> Result<(), std::io::Error> {
            let snapshot: Vec<(String, i64)> = self
                .blocks
                .lock()
                .unwrap()
                .iter()
                .filter(|((repo, _), _)| repo == repo_id)
                .map(|((_, block), (_, modified))| (block.clone(), *modified))
                .collect();
            for (block, modified) in snapshot {
                f(&block, Some(modified));
            }
            Ok(())
        }

        async fn remove_blocks(
            &self,
            repo_id: &str,
            block_ids: &[String],
        ) -> Result<u64, std::io::Error> {
            self.delete_calls
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let mut blocks = self.blocks.lock().unwrap();
            let mut removed = 0u64;
            for block_id in block_ids {
                if blocks
                    .remove(&(repo_id.to_string(), block_id.clone()))
                    .is_some()
                {
                    removed += 1;
                }
            }
            Ok(removed)
        }

        async fn repo_ids(&self) -> Result<Vec<String>, std::io::Error> {
            let mut ids: Vec<String> = self
                .blocks
                .lock()
                .unwrap()
                .keys()
                .map(|(repo, _)| repo.clone())
                .collect();
            ids.sort();
            ids.dedup();
            Ok(ids)
        }

        async fn remove_repo(&self, repo_id: &str) -> Result<bool, std::io::Error> {
            self.repo_removals
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let mut blocks = self.blocks.lock().unwrap();
            let before = blocks.len();
            blocks.retain(|(repo, _), _| repo != repo_id);
            Ok(blocks.len() != before)
        }
    }

    /// A block an upload announced and never committed is removed once its window
    /// passes; one announced recently is left alone even though no commit references
    /// it yet, in both planning modes.
    #[tokio::test]
    async fn pending_ledger_sweeps_expired_and_spares_fresh() {
        let db = setup_gc_test_db(0, 0).await;
        let repos = crate::repository::Repositories::new_for_tests(Arc::new(db.clone()));
        let (_dir, store) = temp_block_store();
        let now = chrono::Utc::now().timestamp();

        insert_commit(&db, "c1", &"a1".repeat(20), now).await;
        insert_fs_object(
            &db,
            &"a1".repeat(20),
            3,
            r#"{"dirents":[],"type":3,"version":1}"#,
        )
        .await;

        let old = store.write_block(REPO, b"old pending").await.unwrap();
        let fresh = store.write_block(REPO, b"fresh pending").await.unwrap();
        repos
            .pending_block
            .mark_written(REPO, std::slice::from_ref(&old), now - 7200)
            .await
            .unwrap();
        repos
            .pending_block
            .mark_written(REPO, std::slice::from_ref(&fresh), now)
            .await
            .unwrap();

        let report = GcManager::garbage_collect_report(
            &repos,
            &store,
            GcPolicy {
                min_block_age_secs: 0,
                verify_head: false,
                upload_activity_ttl: None,
                pending_ttl_secs: 3600,
                ..GcPolicy::new(0)
            },
            None,
        )
        .await
        .expect("gc succeeds");

        assert_eq!(report.expired_pending, 1);
        assert_eq!(report.deleted_blocks, 0);
        assert!(!store.has_block(REPO, &old).await);
        assert!(
            store.has_block(REPO, &fresh).await,
            "a recently announced block must survive the audit"
        );
        assert!(
            repos
                .pending_block
                .written_after(REPO, std::slice::from_ref(&fresh), now - 60)
                .await
                .unwrap()
                .contains(&fresh),
            "its row is still there, so the next pass keeps treating it as fresh"
        );
    }

    /// An incremental schedule turns one pass into an audit when the last one is old
    /// (or unknown), records the time, and goes back to incremental afterwards.
    #[tokio::test]
    async fn incremental_audits_when_due_and_records_the_time() {
        let db = setup_gc_test_db(0, 0).await;
        let repos = crate::repository::Repositories::new_for_tests(Arc::new(db.clone()));
        let (_dir, store) = temp_block_store();
        let now = chrono::Utc::now().timestamp();

        insert_commit(&db, "c1", &"a1".repeat(20), now).await;
        insert_fs_object(
            &db,
            &"a1".repeat(20),
            3,
            r#"{"dirents":[],"type":3,"version":1}"#,
        )
        .await;

        let crashed = store
            .write_block(REPO, b"crashed before commit")
            .await
            .unwrap();
        let policy = GcPolicy {
            mode: GcMode::Incremental,
            audit_interval_secs: 3600,
            pending_ttl_secs: 0,
            verify_head: false,
            upload_activity_ttl: None,
            ..GcPolicy::new(0)
        };

        let first = GcManager::garbage_collect_report(&repos, &store, policy, None)
            .await
            .expect("gc succeeds");
        assert!(first.audited, "no audit has ever run, so this pass is one");
        assert_eq!(first.deleted_blocks, 1);
        assert!(!store.has_block(REPO, &crashed).await);
        assert!(
            repos
                .gc_state
                .get_i64(crate::repository::gc_state::LAST_AUDIT_AT)
                .await
                .unwrap()
                .is_some(),
            "the audit time is recorded"
        );

        let after_audit = store
            .write_block(REPO, b"crash after the audit")
            .await
            .unwrap();
        let second = GcManager::garbage_collect_report(&repos, &store, policy, None)
            .await
            .expect("gc succeeds");
        assert!(!second.audited, "the audit is not due again yet");
        assert_eq!(second.deleted_blocks, 0);
        assert!(
            store.has_block(REPO, &after_audit).await,
            "incremental planning does not enumerate, so it cannot see this one"
        );
    }

    /// A dry run reports the audit it would do and leaves the timestamp alone.
    #[tokio::test]
    async fn a_dry_run_does_not_record_the_audit_time() {
        let db = setup_gc_test_db(0, 0).await;
        let repos = crate::repository::Repositories::new_for_tests(Arc::new(db.clone()));
        let (_dir, store) = temp_block_store();
        let now = chrono::Utc::now().timestamp();

        insert_commit(&db, "c1", &"a1".repeat(20), now).await;
        insert_fs_object(
            &db,
            &"a1".repeat(20),
            3,
            r#"{"dirents":[],"type":3,"version":1}"#,
        )
        .await;
        let crashed = store
            .write_block(REPO, b"crashed before commit")
            .await
            .unwrap();

        let report = GcManager::garbage_collect_report(
            &repos,
            &store,
            GcPolicy {
                mode: GcMode::Incremental,
                audit_interval_secs: 3600,
                pending_ttl_secs: 0,
                verify_head: false,
                upload_activity_ttl: None,
                dry_run: true,
                ..GcPolicy::new(0)
            },
            None,
        )
        .await
        .expect("gc succeeds");

        assert!(report.audited);
        assert_eq!(report.deleted_blocks, 1, "it predicts the deletion");
        assert!(
            store.has_block(REPO, &crashed).await,
            "but nothing is deleted"
        );
        assert!(
            repos
                .gc_state
                .get_i64(crate::repository::gc_state::LAST_AUDIT_AT)
                .await
                .unwrap()
                .is_none(),
            "a dry run must not record that an audit happened"
        );
    }

    /// The audit path must take the age from the enumeration (no per-block probe) and
    /// delete a batch with one call, and the prefix sweep must go through `remove_repo`.
    #[tokio::test]
    async fn audit_uses_the_listed_age_and_batches_deletes() {
        use std::sync::atomic::Ordering;

        let db = setup_gc_test_db(0, 0).await;
        let repos = crate::repository::Repositories::new_for_tests(Arc::new(db.clone()));
        let now = chrono::Utc::now().timestamp();

        // An empty root makes every stored block an orphan.
        insert_commit(&db, "c1", &"a1".repeat(20), now).await;
        insert_fs_object(
            &db,
            &"a1".repeat(20),
            3,
            r#"{"dirents":[],"type":3,"version":1}"#,
        )
        .await;

        let old_one = "b1".repeat(20);
        let old_two = "b2".repeat(20);
        let fresh = "b3".repeat(20);
        let ghost_block = "b4".repeat(20);
        let store = Arc::new(ObjectStoreLike::default());
        store.put(REPO, &old_one, b"old one", now - 7200);
        store.put(REPO, &old_two, b"old two", now - 7200);
        store.put(REPO, &fresh, b"fresh", now);
        store.put("ghost-repo", &ghost_block, b"ghost", now - 7200);
        let backend: DynBlockStorage = store.clone();

        let report = GcManager::garbage_collect_report(
            &repos,
            &backend,
            GcPolicy {
                min_block_age_secs: 3600,
                verify_head: false,
                upload_activity_ttl: None,
                ..GcPolicy::new(3600)
            },
            None,
        )
        .await
        .expect("gc succeeds");

        assert_eq!(report.deleted_blocks, 2);
        assert_eq!(report.skipped_fresh, 1);
        assert_eq!(report.removed_repos, 1);
        assert_eq!(
            store.delete_calls.load(Ordering::Relaxed),
            1,
            "the two old blocks must go out as one batch"
        );
        assert_eq!(
            store.repo_removals.load(Ordering::Relaxed),
            1,
            "the orphan prefix is removed as a prefix, not block by block"
        );
        assert!(!store.has_block(REPO, &old_one).await);
        assert!(!store.has_block(REPO, &old_two).await);
        assert!(store.has_block(REPO, &fresh).await);
        assert!(!store.has_block("ghost-repo", &ghost_block).await);
    }

    /// Repos with both retention settings at 0 are unlimited and must not be touched.
    #[tokio::test]
    async fn test_gc_skips_unlimited_repos() {
        let db = setup_gc_test_db(0, 0).await;
        let repos = crate::repository::Repositories::new_for_tests(Arc::new(db.clone()));
        let (_dir, store) = temp_block_store();

        insert_commit(&db, "c1", "a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1", 3000).await;
        insert_commit(&db, "c2", "a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2", 1000).await;
        insert_fs_object(
            &db,
            "a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1",
            3,
            r#"{"dirents":[],"type":3,"version":1}"#,
        )
        .await;

        let removed = GcManager::garbage_collect(&repos, &store, GcPolicy::immediate())
            .await
            .expect("gc succeeds");
        assert_eq!(removed, 0);
        assert_eq!(count_rows(&db, "commits").await, 2);
        assert_eq!(count_rows(&db, "fs_objects").await, 1);
    }

    /// With only a TTL set, old commits and their unreachable fs objects are pruned.
    #[tokio::test]
    async fn test_gc_prunes_by_ttl() {
        let db = setup_gc_test_db(0, 1).await; // 1-day TTL
        let repos = crate::repository::Repositories::new_for_tests(Arc::new(db.clone()));
        let (_dir, store) = temp_block_store();

        insert_commit(
            &db,
            "c-new",
            "a4a4a4a4a4a4a4a4a4a4a4a4a4a4a4a4a4a4a4a4",
            chrono::Utc::now().timestamp(),
        )
        .await;
        insert_commit(
            &db,
            "c-old",
            "a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5",
            chrono::Utc::now().timestamp() - 172_800,
        )
        .await; // 2 days ago
        insert_fs_object(&db, "a4a4a4a4a4a4a4a4a4a4a4a4a4a4a4a4a4a4a4a4", 3, r#"{"dirents":[{"id":"b1b1b1b1b1b1b1b1b1b1b1b1b1b1b1b1b1b1b1b1","mode":33188,"modifier":"u1","mtime":1000,"name":"a.txt","size":10}],"type":3,"version":1}"#).await;
        insert_fs_object(
            &db,
            "b1b1b1b1b1b1b1b1b1b1b1b1b1b1b1b1b1b1b1b1",
            1,
            r#"{"block_ids":["aaaa"],"size":10,"type":1,"version":1}"#,
        )
        .await;
        insert_fs_object(
            &db,
            "a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5",
            3,
            r#"{"dirents":[],"type":3,"version":1}"#,
        )
        .await;

        let removed = GcManager::garbage_collect(&repos, &store, GcPolicy::immediate())
            .await
            .expect("gc succeeds");
        // a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5 is orphaned (only reachable from the pruned commit).
        assert_eq!(removed, 1);
        assert_eq!(count_rows(&db, "commits").await, 1);
        assert_eq!(count_rows(&db, "fs_objects").await, 2);
    }

    /// An orphan block is deleted, while a block referenced by a live file survives.
    #[tokio::test]
    async fn test_gc_deletes_orphan_block_keeps_referenced_block() {
        let db = setup_gc_test_db(0, 0).await; // unlimited → nothing pruned
        let repos = crate::repository::Repositories::new_for_tests(Arc::new(db.clone()));
        let (_dir, store) = temp_block_store();

        let kept_id = store.write_block(REPO, b"kept content").await.unwrap();
        let orphan_id = store.write_block(REPO, b"orphan content").await.unwrap();

        insert_commit(&db, "c1", "a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1", 3000).await;
        insert_fs_object(
            &db,
            "a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1",
            3,
            r#"{"dirents":[{"id":"b1b1b1b1b1b1b1b1b1b1b1b1b1b1b1b1b1b1b1b1","mode":33188,"modifier":"u1","mtime":1000,"name":"a.txt","size":10}],"type":3,"version":1}"#,
        )
        .await;
        let kept_json = format!(r#"{{"block_ids":["{kept_id}"],"size":10,"type":1,"version":1}}"#);
        insert_fs_object(
            &db,
            "b1b1b1b1b1b1b1b1b1b1b1b1b1b1b1b1b1b1b1b1",
            1,
            &kept_json,
        )
        .await;

        let removed = GcManager::garbage_collect(&repos, &store, GcPolicy::immediate())
            .await
            .expect("gc succeeds");
        assert_eq!(removed, 1, "only the orphan block should be removed");
        assert!(
            store.has_block(REPO, &kept_id).await,
            "referenced block must survive"
        );
        assert!(
            !store.has_block(REPO, &orphan_id).await,
            "orphan block must be deleted"
        );
    }

    /// A shared block survives as long as any retained commit still references it.
    #[tokio::test]
    async fn test_gc_keeps_block_referenced_by_retained_commit() {
        let db = setup_gc_test_db(1, 0).await; // history_limit = 1
        let repos = crate::repository::Repositories::new_for_tests(Arc::new(db.clone()));
        let (_dir, store) = temp_block_store();

        let shared_id = store.write_block(REPO, b"shared content").await.unwrap();
        let orphan_id = store.write_block(REPO, b"orphan content").await.unwrap();
        let shared_json =
            format!(r#"{{"block_ids":["{shared_id}"],"size":10,"type":1,"version":1}}"#);

        // Older commit (pruned by history_limit=1) references the block via b1b1b1b1b1b1b1b1b1b1b1b1b1b1b1b1b1b1b1b1.
        insert_commit(&db, "c1", "a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1", 1000).await;
        insert_fs_object(
            &db,
            "a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1",
            3,
            r#"{"dirents":[{"id":"b1b1b1b1b1b1b1b1b1b1b1b1b1b1b1b1b1b1b1b1","mode":33188,"modifier":"u1","mtime":1000,"name":"a.txt","size":10}],"type":3,"version":1}"#,
        )
        .await;
        insert_fs_object(
            &db,
            "b1b1b1b1b1b1b1b1b1b1b1b1b1b1b1b1b1b1b1b1",
            1,
            &shared_json,
        )
        .await;

        // Retained head commit still references the block via b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2.
        insert_commit(&db, "c2", "a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2", 3000).await;
        insert_fs_object(
            &db,
            "a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2",
            3,
            r#"{"dirents":[{"id":"b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2","mode":33188,"modifier":"u1","mtime":1000,"name":"b.txt","size":10}],"type":3,"version":1}"#,
        )
        .await;
        insert_fs_object(
            &db,
            "b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2",
            1,
            &shared_json,
        )
        .await;

        let removed = GcManager::garbage_collect(&repos, &store, GcPolicy::immediate())
            .await
            .expect("gc succeeds");
        // 2 orphaned fs rows (a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1, b1b1b1b1b1b1b1b1b1b1b1b1b1b1b1b1b1b1b1b1) + 1 orphan block.
        assert_eq!(removed, 3);
        // Shared block is still referenced by c2's b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2 → must survive.
        assert!(
            store.has_block(REPO, &shared_id).await,
            "shared block must survive"
        );
        assert!(
            !store.has_block(REPO, &orphan_id).await,
            "orphan block must be deleted"
        );
    }

    /// A repo with no head (a commit uploaded but never promoted) has no current
    /// tree to protect: a TTL that retains no commits prunes every fs_object.
    #[tokio::test]
    async fn test_gc_prunes_all_when_no_commits_kept() {
        let db = setup_gc_test_db(0, 1).await; // 1-day TTL
        let repos = crate::repository::Repositories::new_for_tests(Arc::new(db.clone()));
        let (_dir, store) = temp_block_store();

        let old = chrono::Utc::now().timestamp() - 172_800; // 2 days ago
        insert_commit(&db, "c1", "a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1", old).await;
        insert_commit(&db, "c2", "a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2", old).await;
        insert_fs_object(
            &db,
            "a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1",
            3,
            r#"{"dirents":[],"type":3,"version":1}"#,
        )
        .await;
        insert_fs_object(
            &db,
            "a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2",
            3,
            r#"{"dirents":[],"type":3,"version":1}"#,
        )
        .await;

        let removed = GcManager::garbage_collect(&repos, &store, GcPolicy::immediate())
            .await
            .expect("gc succeeds");
        assert_eq!(removed, 2);
        assert_eq!(count_rows(&db, "commits").await, 0);
        assert_eq!(count_rows(&db, "fs_objects").await, 0);
    }

    /// Backdate a block's mtime to exercise the grace window. Blocks are content-addressed
    /// and the on-disk path is private to the store, so the file is found by name.
    fn backdate_block(dir: &std::path::Path, block_id: &str, age_secs: u64) {
        let mut stack = vec![dir.to_path_buf()];
        while let Some(current) = stack.pop() {
            for entry in std::fs::read_dir(&current).unwrap() {
                let entry = entry.unwrap();
                let path = entry.path();
                if path.is_dir() {
                    stack.push(path);
                } else if entry.file_name() == block_id {
                    let when =
                        std::time::SystemTime::now() - std::time::Duration::from_secs(age_secs);
                    std::fs::File::options()
                        .write(true)
                        .open(&path)
                        .unwrap()
                        .set_modified(when)
                        .unwrap();
                    return;
                }
            }
        }
        panic!("block {block_id} not found under {}", dir.display());
    }

    /// The grace window spares a fresh block (it may belong to an in-flight upload)
    /// while an older orphan is still deleted.
    #[tokio::test]
    async fn test_gc_grace_window_spares_fresh_blocks_only() {
        let db = setup_gc_test_db(0, 0).await; // unlimited → nothing pruned
        let repos = crate::repository::Repositories::new_for_tests(Arc::new(db.clone()));
        let (dir, store) = temp_block_store();

        let kept_id = store.write_block(REPO, b"kept content").await.unwrap();
        let fresh_id = store.write_block(REPO, b"fresh orphan").await.unwrap();
        let stale_id = store.write_block(REPO, b"stale orphan").await.unwrap();
        // Age one orphan past the window; the other keeps its just-written mtime.
        backdate_block(dir.path(), &stale_id, 2 * 3600);

        insert_commit(&db, "c1", "a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1", 3000).await;
        insert_fs_object(
            &db,
            "a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1",
            3,
            r#"{"dirents":[{"id":"b1b1b1b1b1b1b1b1b1b1b1b1b1b1b1b1b1b1b1b1","mode":33188,"modifier":"u1","mtime":1000,"name":"a.txt","size":10}],"type":3,"version":1}"#,
        )
        .await;
        let kept_json = format!(r#"{{"block_ids":["{kept_id}"],"size":10,"type":1,"version":1}}"#);
        insert_fs_object(
            &db,
            "b1b1b1b1b1b1b1b1b1b1b1b1b1b1b1b1b1b1b1b1",
            1,
            &kept_json,
        )
        .await;

        // This test exercises the block-age window, not the upload lease; a
        // process-global activity mark from a parallel test must not skip it.
        let policy = GcPolicy {
            upload_activity_ttl: None,
            ..GcPolicy::new(3600)
        };
        let removed = GcManager::garbage_collect(&repos, &store, policy)
            .await
            .expect("gc succeeds");
        assert_eq!(removed, 1, "only the orphan past the window is removable");
        assert!(
            store.has_block(REPO, &kept_id).await,
            "referenced block must survive"
        );
        assert!(
            store.has_block(REPO, &fresh_id).await,
            "a block written around the snapshot must survive the grace window"
        );
        assert!(
            !store.has_block(REPO, &stale_id).await,
            "an orphan older than the window must still be deleted"
        );

        // Disabling the window makes the spared orphan deletable.
        let removed = GcManager::garbage_collect(&repos, &store, GcPolicy::immediate())
            .await
            .expect("gc succeeds");
        assert_eq!(removed, 1);
        assert!(
            !store.has_block(REPO, &fresh_id).await,
            "with no window the fresh orphan is collected"
        );
    }

    /// A pass with a context stops as soon as the run is asked to stop.
    #[tokio::test]
    async fn an_interruptible_pass_stops_at_a_checkpoint() {
        let db = setup_gc_test_db(0, 0).await;
        let repos = crate::repository::Repositories::new_for_tests(Arc::new(db.clone()));
        let (_dir, store) = temp_block_store();
        store.write_block(REPO, b"orphan content").await.unwrap();
        insert_commit(&db, "c1", "a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1", 3000).await;
        insert_fs_object(
            &db,
            "a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1",
            3,
            r#"{"dirents":[],"type":3,"version":1}"#,
        )
        .await;

        let ctx = crate::tasks::test_support::cancelled_context();
        let result =
            GcManager::garbage_collect_with(&repos, &store, GcPolicy::immediate(), Some(&ctx))
                .await;
        match result {
            Err(AppError::OperationFailed(message)) => {
                assert_eq!(
                    message, ABORTED,
                    "the pass reports the checkpoint it stopped at"
                )
            }
            other => panic!("expected the pass to abort, got {other:?}"),
        }
    }

    /// The head re-check compares the live row against the snapshot and treats a
    /// vanished repository as moved.
    #[tokio::test]
    async fn test_repo_head_moved_detects_changes_since_the_snapshot() {
        let db = setup_gc_test_db(0, 0).await;
        let repos = crate::repository::Repositories::new_for_tests(Arc::new(db.clone()));

        let snapshot = repos
            .repo
            .find_all()
            .await
            .unwrap()
            .pop()
            .expect("seeded repo");
        assert!(
            !GcManager::repo_head_moved(&repos, &snapshot).await.unwrap(),
            "an unchanged head must not look moved"
        );

        // A commit landing after the snapshot moves the head.
        db.execute_raw(Statement::from_string(
            DatabaseBackend::Sqlite,
            "UPDATE repos SET head_commit_id = 'cccccccccccccccccccccccccccccccccccccccc'"
                .to_string(),
        ))
        .await
        .unwrap();
        assert!(
            GcManager::repo_head_moved(&repos, &snapshot).await.unwrap(),
            "a head that moved since the snapshot must be detected"
        );

        // A repository deleted mid-pass is no longer this pass's to judge.
        db.execute_raw(Statement::from_string(
            DatabaseBackend::Sqlite,
            "DELETE FROM repos".to_string(),
        ))
        .await
        .unwrap();
        assert!(
            GcManager::repo_head_moved(&repos, &snapshot).await.unwrap(),
            "a deleted repository counts as moved"
        );
    }
}
