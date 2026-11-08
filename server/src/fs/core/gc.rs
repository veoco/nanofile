use crate::fs::core::tree::read_fs_file_data_batch;
use crate::fs::core::upload_activity;
use crate::repository::Repositories;
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
}

impl GcPolicy {
    /// The production policy: age grace window on, head re-checked, and recent
    /// uploads left alone.
    pub fn new(min_block_age_secs: u64) -> Self {
        Self {
            min_block_age_secs,
            verify_head: true,
            upload_activity_ttl: Some(upload_activity::UPLOAD_ACTIVITY_TTL),
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
        }
    }
}

/// How many block deletions may run between checkpoints. Bounded by count rather
/// than time so the interval is predictable; a deletion is an unlink, so batches
/// are short.
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
        let now = chrono::Utc::now().timestamp();
        let all_repos = repos.repo.find_all().await?;
        let mut removed = 0u64;

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

            // GC needs every commit, so it passes `None` to fetch all rows.
            let commits = repos
                .commit
                .find_by_repo_id_ordered_by_ctime_desc(&repo_model.id, None)
                .await?;
            if commits.is_empty() {
                continue;
            }

            // Snapshot the FS rows this pass is allowed to delete. A row inserted
            // after this point may belong to an upload that started during the
            // pass, so it must never be a candidate here.
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

            // Block ids referenced by this repository's retained history; rebuilt
            // per repository because there is no cross-repository sharing.
            let mut alive_blocks: std::collections::HashSet<String> =
                std::collections::HashSet::new();
            let reachable = Self::collect_repo_alive_blocks(
                repos,
                &repo_model.id,
                &commits,
                &keep,
                &mut alive_blocks,
            )
            .await?;

            // Only repos with retention limits are pruned; an unlimited repo's
            // history is all live.
            if let Some(fs_snapshot) = &fs_snapshot {
                // An upload may have started after the early check. Re-check right
                // before the irreversible deletes, and skip when the head moved
                // after the snapshot this pass works from.
                if let Some(ttl) = policy.upload_activity_ttl
                    && upload_activity::is_active(&repo_model.id, ttl)
                {
                    tracing::debug!(
                        repo_id = %repo_model.id,
                        "skipping history prune: recent upload activity"
                    );
                    continue;
                }
                if policy.verify_head && Self::repo_head_moved(repos, repo_model).await? {
                    tracing::debug!(
                        repo_id = %repo_model.id,
                        "skipping history prune: the repository head moved during this GC pass"
                    );
                    continue;
                }
                removed += Self::prune_repo(repos, &commits, &keep, reachable, fs_snapshot).await?;
            }

            // Stream the disk blocks so the full list is never materialised; only
            // the orphan subset (usually tiny) is collected for deletion.
            block_store.invalidate_exists_cache();
            // A `'static` callback can't borrow, so the live set and orphan
            // collector are shared through Arc rather than borrowed.
            let alive_blocks = std::sync::Arc::new(alive_blocks);
            let orphan_blocks = std::sync::Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
            let (alive_ref, orphan_ref) = (alive_blocks.clone(), orphan_blocks.clone());
            block_store
                .for_each_block_in_repo(
                    &repo_model.id,
                    Box::new(move |id| {
                        if !alive_ref.contains(id) {
                            orphan_ref.lock().unwrap().push(id.to_string());
                        }
                    }),
                )
                .await?;
            let orphan_blocks = orphan_blocks.lock().unwrap().clone();

            // An upload that started while the blocks were being enumerated may
            // reuse an old block this pass classified as an orphan; leave the
            // repository's blocks for a later pass when activity is recent.
            if let Some(ttl) = policy.upload_activity_ttl
                && upload_activity::is_active(&repo_model.id, ttl)
            {
                tracing::debug!(
                    repo_id = %repo_model.id,
                    orphans = orphan_blocks.len(),
                    "skipping block deletion: recent upload activity"
                );
                continue;
            }

            // A commit that landed since the snapshot can reference a block this pass
            // classified as an orphan, so re-read the head and leave the repository
            // alone when it moved. Placed as late as possible to keep the window narrow.
            if policy.verify_head && Self::repo_head_moved(repos, repo_model).await? {
                tracing::warn!(
                    repo_id = %repo_model.id,
                    orphans = orphan_blocks.len(),
                    "skipping block deletion: the repository head moved during this GC pass"
                );
                continue;
            }

            let mut deleted = 0u64;
            let mut skipped_fresh = 0u64;
            for (index, id) in orphan_blocks.iter().enumerate() {
                // Many orphans can make a long tail, so check in periodically
                // rather than only between repositories.
                if index % CHECKPOINT_EVERY_BLOCKS == 0 {
                    checkpoint(ctx).await?;
                }
                // Second guard: a block written at or after the snapshot may belong to
                // an upload whose commit is not visible yet — leave it for a later pass.
                if policy.min_block_age_secs > 0
                    && block_store
                        .block_modified_secs(&repo_model.id, id)
                        .await?
                        .is_some_and(|m| now.saturating_sub(m) < policy.min_block_age_secs as i64)
                {
                    skipped_fresh += 1;
                    continue;
                }
                block_store.remove_block(&repo_model.id, id).await?;
                deleted += 1;
            }
            if skipped_fresh > 0 {
                tracing::debug!(
                    repo_id = %repo_model.id,
                    skipped_fresh,
                    min_block_age_secs = policy.min_block_age_secs,
                    "kept blocks younger than the GC grace window"
                );
            }
            removed += deleted;
        }

        removed += Self::sweep_orphan_repo_dirs(repos, block_store).await?;

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

    /// Remove block directories whose repository no longer exists.
    ///
    /// Deleting a repository removes its rows (cascading to commits and fs_objects)
    /// but leaves its block files, which would otherwise stay on disk forever.
    /// Directories whose id is still in the trash table are kept: restoring a
    /// deleted library reuses the same repository id.
    async fn sweep_orphan_repo_dirs(
        repos: &Repositories,
        block_store: &DynBlockStorage,
    ) -> Result<u64, AppError> {
        let mut removed = 0u64;
        for (repo_id, dir) in block_store.repo_dirs().await? {
            if repos.repo.find_by_id(&repo_id).await?.is_some() {
                continue;
            }
            if repos.deleted_repo.find_by_id(&repo_id).await?.is_some() {
                continue;
            }
            tracing::info!(
                repo_id = %repo_id,
                dir = %dir.display(),
                "removing block directory of a repository that no longer exists"
            );
            match tokio::fs::remove_dir_all(&dir).await {
                Ok(()) => removed += 1,
                Err(e) => tracing::warn!(
                    dir = %dir.display(),
                    "failed to remove orphan block directory: {e}"
                ),
            }
        }
        Ok(removed)
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
        // `prune_repo` can reuse it instead of re-walking the same trees.
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

    /// Prune one repo's history per its retention settings, deleting fs_object and
    /// commit rows no longer reachable from any retained commit. The caller
    /// supplies the full commit list, the kept-set, and the FS-object snapshot
    /// taken at the start of the pass.
    ///
    /// Only rows from `fs_snapshot` are candidates: a row inserted after the
    /// snapshot may belong to an upload that is still in flight.
    async fn prune_repo(
        repos: &Repositories,
        commits: &[commit::Model],
        keep: &std::collections::HashSet<i64>,
        active_fs_ids: std::collections::HashSet<[u8; 20]>,
        fs_snapshot: &[(i64, [u8; 20])],
    ) -> Result<u64, AppError> {
        if keep.len() == commits.len() {
            return Ok(0);
        }

        // Delete no-longer-reachable fs objects from the pass snapshot. Only
        // project (id, fs_id): the `data` JSON can be large and is irrelevant for
        // computing orphans.
        let inactive_ids: Vec<i64> = fs_snapshot
            .iter()
            .filter(|(_, fs_id)| !active_fs_ids.contains(fs_id))
            .map(|(id, _)| *id)
            .collect();
        let removed = inactive_ids.len();
        if !inactive_ids.is_empty() {
            repos.fs_object.delete_many_by_ids(inactive_ids).await?;
        }

        let pruned_ids: Vec<i64> = commits
            .iter()
            .filter(|c| !keep.contains(&c.id))
            .map(|c| c.id)
            .collect();
        if !pruned_ids.is_empty() {
            repos.commit.delete_many_by_ids(pruned_ids).await?;
        }

        Ok(removed as u64)
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

    /// `prune_repo` only considers the FS-object snapshot it was given: a row
    /// inserted after the pass started must survive even when it is unreachable.
    #[tokio::test]
    async fn test_prune_repo_deletes_only_snapshot_rows() {
        let repo = "gc-snapshot-test";
        let db = setup_gc_test_db_for(repo, 1, 0).await;
        let repos = crate::repository::Repositories::new_for_tests(Arc::new(db.clone()));

        let old_root = "f1".repeat(20);
        let head_root = "f2".repeat(20);
        let late_root = "f3".repeat(20);
        insert_commit_for(&db, repo, &"f4".repeat(20), &old_root, 1000).await;
        insert_commit_for(&db, repo, &"f5".repeat(20), &head_root, 2000).await;
        for root in [&old_root, &head_root, &late_root] {
            insert_fs_object_for(&db, repo, root, 3, r#"{"dirents":[],"type":3,"version":1}"#)
                .await;
        }

        // Snapshot taken before `late_root` existed.
        let snapshot = repos
            .fs_object
            .find_ids_and_fs_ids_by_repo_id(repo)
            .await
            .unwrap()
            .into_iter()
            .filter(|(_, fs_id)| *fs_id != GcManager::decode_sha1_hex(&late_root))
            .collect::<Vec<_>>();
        assert_eq!(snapshot.len(), 2);

        let commits = repos
            .commit
            .find_by_repo_id_ordered_by_ctime_desc(repo, None)
            .await
            .unwrap();
        let keep = std::collections::HashSet::from([commits[0].id]);
        let removed = GcManager::prune_repo(
            &repos,
            &commits,
            &keep,
            std::collections::HashSet::new(),
            &snapshot,
        )
        .await
        .expect("prune succeeds");

        assert_eq!(removed, 2, "both snapshot FS rows are unreachable");
        assert!(
            repos
                .fs_object
                .find_by_repo_and_fs_id(repo, &late_root)
                .await
                .unwrap()
                .is_some(),
            "a row inserted after the snapshot must survive"
        );
        assert_eq!(count_rows(&db, "fs_objects").await, 1);
        assert_eq!(count_rows(&db, "commits").await, 1);
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
