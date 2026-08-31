use crate::fs::core::tree::read_fs_file_data_batch;
use crate::repository::Repositories;
use base::common::{FsDirData, S_IFDIR, SEAF_METADATA_TYPE_DIR};
use base::error::AppError;
use infra::entity::{commit, repo};
use infra::storage::DynBlockStorage;

const SECS_PER_DAY: i64 = 86_400;

pub struct GcManager;

impl GcManager {
    /// Garbage-collect every repo that has history retention limits set, and
    /// delete block files no longer referenced by any retained commit anywhere.
    ///
    /// Blocks are content-addressed and shared across files/repos, so a block is
    /// deletable only when it is unreachable from every retained commit's FS
    /// tree (current head + `history_limit` / `history_ttl_days` retained
    /// history, across all repos). Orphan blocks are `list_blocks` minus that
    /// global live set.
    pub async fn garbage_collect(
        repos: &Repositories,
        block_store: &DynBlockStorage,
    ) -> Result<u64, AppError> {
        let now = chrono::Utc::now().timestamp();
        let all_repos = repos.repo.find_all().await?;

        let mut alive_blocks: std::collections::HashSet<[u8; 20]> =
            std::collections::HashSet::new();
        let mut removed = 0u64;

        for repo_model in &all_repos {
            // GC needs every commit (history_limit + TTL + reachable-set
            // collection), so it passes `None` to fetch all rows.
            let commits = repos
                .commit
                .find_by_repo_id_ordered_by_ctime_desc(&repo_model.id, None)
                .await?;
            if commits.is_empty() {
                continue;
            }

            let keep = Self::compute_keep_commits(
                &commits,
                repo_model.history_limit,
                repo_model.history_ttl_days,
                now,
            );
            let reachable = Self::collect_repo_alive_blocks(
                repos,
                &repo_model.id,
                &commits,
                &keep,
                &mut alive_blocks,
            )
            .await?;

            // Prune only repos that have retention limits; unlimited repos are
            // left untouched (all of their history is live).
            if repo_model.history_limit != 0 || repo_model.history_ttl_days != 0 {
                removed += Self::prune_repo(repos, repo_model, &commits, &keep, reachable).await?;
            }
        }

        // Delete blocks no longer referenced by any retained commit anywhere.
        // Stream disk blocks so the full on-disk list is never materialised;
        // only the orphan subset (usually tiny) is collected for deletion.
        block_store.invalidate_exists_cache();
        // The callback is a `'static` trait object, so the live set and orphan
        // collector are shared through Arc rather than borrowed.
        let alive_blocks = std::sync::Arc::new(alive_blocks);
        let orphan_blocks = std::sync::Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
        let (alive_ref, orphan_ref) = (alive_blocks.clone(), orphan_blocks.clone());
        block_store
            .for_each_block(Box::new(move |id| {
                if !alive_ref.contains(&Self::decode_block_id(id)) {
                    orphan_ref.lock().unwrap().push(id.to_string());
                }
            }))
            .await?;
        let orphan_blocks = orphan_blocks.lock().unwrap().clone();
        for id in &orphan_blocks {
            block_store.remove_block(id).await?;
        }
        removed += orphan_blocks.len() as u64;

        Ok(removed)
    }

    /// Compute the set of commit ids (by DB primary key) to retain, newest-first.
    ///
    /// `history_limit > 0` keeps the newest N commits; `history_ttl_days > 0`
    /// keeps any commit newer than the cutoff. With both at 0 the repo is
    /// unlimited, so **every** commit (and every block reachable from it) is
    /// retained — otherwise its blocks would be mis-identified as orphans.
    fn compute_keep_commits(
        commits: &[commit::Model],
        history_limit: i32,
        history_ttl_days: i32,
        now: i64,
    ) -> std::collections::HashSet<i64> {
        let mut keep = std::collections::HashSet::new();
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
    /// commits of a single repo into `alive_blocks`. Run for every repo (even
    /// unlimited ones) so cross-repo shared blocks are never deleted.
    async fn collect_repo_alive_blocks(
        repos: &Repositories,
        repo_id: &str,
        commits: &[commit::Model],
        keep: &std::collections::HashSet<i64>,
        alive_blocks: &mut std::collections::HashSet<[u8; 20]>,
    ) -> Result<std::collections::HashSet<String>, AppError> {
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
        let ids: Vec<String> = reachable.iter().cloned().collect();
        let files = read_fs_file_data_batch(repos, repo_id, &ids).await?;
        for file in files.values() {
            for block_id in &file.block_ids {
                alive_blocks.insert(Self::decode_block_id(block_id));
            }
        }
        Ok(reachable)
    }

    /// Prune one repo's history according to its retention settings, deleting
    /// fs_object and commit rows that are no longer reachable from any retained
    /// commit. Runs only for repos with retention limits; the caller supplies
    /// the full commit list and the already-computed kept-set.
    async fn prune_repo(
        repos: &Repositories,
        repo_model: &repo::Model,
        commits: &[commit::Model],
        keep: &std::collections::HashSet<i64>,
        active_fs_ids: std::collections::HashSet<String>,
    ) -> Result<u64, AppError> {
        if keep.len() == commits.len() {
            return Ok(0);
        }

        // Delete fs objects of this repo that are no longer reachable. Only
        // project (id, fs_id) — the `data` JSON can be large and is irrelevant
        // for computing orphans.
        let all_fs = repos
            .fs_object
            .find_ids_and_fs_ids_by_repo_id(&repo_model.id)
            .await?;
        let inactive_ids: Vec<i64> = all_fs
            .iter()
            .filter(|(_, fs_id)| !active_fs_ids.contains(fs_id))
            .map(|(id, _)| *id)
            .collect();
        let removed = inactive_ids.len();
        if !inactive_ids.is_empty() {
            repos.fs_object.delete_many_by_ids(inactive_ids).await?;
        }

        // Delete the pruned commit rows.
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

    /// Decode a 40-char hex block id to its raw 20-byte SHA-1, so the live set
    /// can be stored compactly. `list_blocks` / `for_each_block` only yield
    /// validated 40-hex ids, so this cannot fail; a corrupt id decodes to
    /// all-zeroes and is treated as an orphan.
    fn decode_block_id(hex_str: &str) -> [u8; 20] {
        let mut buf = [0u8; 20];
        let _ = hex::decode_to_slice(hex_str, &mut buf);
        buf
    }

    /// Breadth-first walk from `root_id`, adding every reachable fs_id to
    /// `collected`. Directory objects are fetched level by level in a single
    /// batched query instead of one query per object.
    async fn collect_fs_ids(
        repos: &Repositories,
        repo_id: &str,
        root_id: &str,
        collected: &mut std::collections::HashSet<String>,
    ) -> Result<(), AppError> {
        if collected.contains(root_id) {
            return Ok(());
        }
        collected.insert(root_id.to_string());

        let mut frontier = vec![root_id.to_string()];
        while !frontier.is_empty() {
            // `fetch_fs_object_map` chunks the IN list to stay under SQLite's
            // variable limit, unlike `find_by_repo_and_fs_ids` which does not.
            let objs =
                crate::fs::core::tree::fetch_fs_object_map(repos, repo_id, &frontier).await?;
            let mut next = Vec::new();
            for obj in objs.values() {
                if obj.obj_type == SEAF_METADATA_TYPE_DIR as i8 {
                    let dir_data: FsDirData = serde_json::from_str(&obj.data)
                        .map_err(|e| AppError::internal(e.to_string()))?;
                    for entry in &dir_data.dirents {
                        let is_new = collected.insert(entry.id.clone());
                        // Only directories have children to recurse into; use
                        // the dirent's `mode` bit so file objects are never
                        // fetched just to discover they aren't directories.
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
    use sea_orm::{ConnectionTrait, Database, DatabaseBackend, Statement};
    use std::sync::Arc;

    /// A throwaway filesystem-backed block store. Returns the `TempDir` so it
    /// stays alive for the whole test.
    fn temp_block_store() -> (tempfile::TempDir, DynBlockStorage) {
        let dir = tempfile::tempdir().unwrap();
        let store = infra::storage::new_block_store(dir.path());
        (dir, store)
    }

    /// In-memory SQLite with the minimal schema + a seeded repo/commits/
    /// fs_objects for GC.
    async fn setup_gc_test_db(
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
                version TINYINT NOT NULL DEFAULT 1
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
                 VALUES ('test-repo', 'test', '', 1, 0, 0, '', 'rw', 0, 0, 0, 1, {history_limit}, {history_ttl_days})"
            ),
        ))
        .await
        .unwrap();

        db
    }

    /// Insert a commit row.
    async fn insert_commit(
        db: &sea_orm::DatabaseConnection,
        commit_id: &str,
        root_id: &str,
        ctime: i64,
    ) {
        db.execute_raw(Statement::from_string(
            DatabaseBackend::Sqlite,
            format!(
                "INSERT INTO commits (repo_id, commit_id, root_id, creator_name, ctime, version) \
                 VALUES ('test-repo', '{commit_id}', '{root_id}', 'u1', {ctime}, 1)"
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
        db.execute_raw(Statement::from_string(
            DatabaseBackend::Sqlite,
            format!(
                "INSERT INTO fs_objects (repo_id, fs_id, obj_type, data) \
                 VALUES ('test-repo', '{fs_id}', {obj_type}, '{data}')"
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

    /// GC with `history_limit = 1` should keep the newest commit, drop the
    /// older commits, and delete fs objects that are no longer reachable from
    /// any kept commit.
    #[tokio::test]
    async fn test_gc_prunes_old_history_and_orphaned_fs_objects() {
        let db = setup_gc_test_db(1, 0).await;
        let repos = crate::repository::Repositories::new(Arc::new(db.clone()));
        let (_dir, store) = temp_block_store();

        // Three commits, newest first by ctime:
        //   c1 (root-c1 → fileA), c2 (root-c2), c3 (root-c3 → fileB)
        insert_commit(&db, "c1", "root-c1", 3000).await;
        insert_commit(&db, "c2", "root-c2", 2000).await;
        insert_commit(&db, "c3", "root-c3", 1000).await;

        // root-c1 and root-c3 are directories referencing fileA / fileB.
        insert_fs_object(&db, "root-c1", 3, r#"{"dirents":[{"id":"fileA","mode":33188,"modifier":"u1","mtime":1000,"name":"a.txt","size":10}],"type":3,"version":1}"#).await;
        insert_fs_object(
            &db,
            "fileA",
            1,
            r#"{"block_ids":["aaaa"],"size":10,"type":1,"version":1}"#,
        )
        .await;
        insert_fs_object(&db, "root-c3", 3, r#"{"dirents":[{"id":"fileB","mode":33188,"modifier":"u1","mtime":1000,"name":"b.txt","size":20}],"type":3,"version":1}"#).await;
        insert_fs_object(
            &db,
            "fileB",
            1,
            r#"{"block_ids":["bbbb"],"size":20,"type":1,"version":1}"#,
        )
        .await;

        let removed = GcManager::garbage_collect(&repos, &store)
            .await
            .expect("gc succeeds");
        // Only the two fs objects reachable solely from c3 are orphaned.
        assert_eq!(removed, 2);

        // Newest commit retained; c2/c3 pruned.
        assert_eq!(count_rows(&db, "commits").await, 1);
        // root-c1 + fileA remain; root-c3 + fileB deleted.
        assert_eq!(count_rows(&db, "fs_objects").await, 2);
    }

    /// Repos with both retention settings at 0 are unlimited and must be left
    /// untouched — even when they have multiple old commits.
    #[tokio::test]
    async fn test_gc_skips_unlimited_repos() {
        let db = setup_gc_test_db(0, 0).await;
        let repos = crate::repository::Repositories::new(Arc::new(db.clone()));
        let (_dir, store) = temp_block_store();

        insert_commit(&db, "c1", "root-c1", 3000).await;
        insert_commit(&db, "c2", "root-c2", 1000).await;
        insert_fs_object(&db, "root-c1", 3, r#"{"dirents":[],"type":3,"version":1}"#).await;

        let removed = GcManager::garbage_collect(&repos, &store)
            .await
            .expect("gc succeeds");
        assert_eq!(removed, 0);
        assert_eq!(count_rows(&db, "commits").await, 2);
        assert_eq!(count_rows(&db, "fs_objects").await, 1);
    }

    /// With only a TTL set, commits older than the window are pruned and their
    /// unreachable fs objects removed.
    #[tokio::test]
    async fn test_gc_prunes_by_ttl() {
        let db = setup_gc_test_db(0, 1).await; // 1-day TTL
        let repos = crate::repository::Repositories::new(Arc::new(db.clone()));
        let (_dir, store) = temp_block_store();

        // now (today) is used by GC; old commit is > 1 day old, new one is recent.
        insert_commit(&db, "c-new", "root-new", chrono::Utc::now().timestamp()).await;
        insert_commit(
            &db,
            "c-old",
            "root-old",
            chrono::Utc::now().timestamp() - 172_800,
        )
        .await; // 2 days ago
        insert_fs_object(&db, "root-new", 3, r#"{"dirents":[{"id":"fileA","mode":33188,"modifier":"u1","mtime":1000,"name":"a.txt","size":10}],"type":3,"version":1}"#).await;
        insert_fs_object(
            &db,
            "fileA",
            1,
            r#"{"block_ids":["aaaa"],"size":10,"type":1,"version":1}"#,
        )
        .await;
        insert_fs_object(&db, "root-old", 3, r#"{"dirents":[],"type":3,"version":1}"#).await;

        let removed = GcManager::garbage_collect(&repos, &store)
            .await
            .expect("gc succeeds");
        // root-old is orphaned (only reachable from the pruned commit).
        assert_eq!(removed, 1);
        assert_eq!(count_rows(&db, "commits").await, 1);
        assert_eq!(count_rows(&db, "fs_objects").await, 2);
    }

    /// An orphan block (not referenced by any surviving file) is deleted, while
    /// a block referenced by a live file object survives.
    #[tokio::test]
    async fn test_gc_deletes_orphan_block_keeps_referenced_block() {
        let db = setup_gc_test_db(0, 0).await; // unlimited → nothing pruned
        let repos = crate::repository::Repositories::new(Arc::new(db.clone()));
        let (_dir, store) = temp_block_store();

        let kept_id = store.write_block(b"kept content").await.unwrap();
        let orphan_id = store.write_block(b"orphan content").await.unwrap();

        insert_commit(&db, "c1", "root-c1", 3000).await;
        insert_fs_object(
            &db,
            "root-c1",
            3,
            r#"{"dirents":[{"id":"fileA","mode":33188,"modifier":"u1","mtime":1000,"name":"a.txt","size":10}],"type":3,"version":1}"#,
        )
        .await;
        let kept_json = format!(r#"{{"block_ids":["{kept_id}"],"size":10,"type":1,"version":1}}"#);
        insert_fs_object(&db, "fileA", 1, &kept_json).await;

        let removed = GcManager::garbage_collect(&repos, &store)
            .await
            .expect("gc succeeds");
        assert_eq!(removed, 1, "only the orphan block should be removed");
        assert!(
            store.has_block(&kept_id).await,
            "referenced block must survive"
        );
        assert!(
            !store.has_block(&orphan_id).await,
            "orphan block must be deleted"
        );
    }

    /// A block referenced by a retained commit's file is not deleted even when
    /// another (older) commit that also referenced it is pruned — i.e. shared
    /// blocks survive as long as any final reachable reference remains.
    #[tokio::test]
    async fn test_gc_keeps_block_referenced_by_retained_commit() {
        let db = setup_gc_test_db(1, 0).await; // history_limit = 1
        let repos = crate::repository::Repositories::new(Arc::new(db.clone()));
        let (_dir, store) = temp_block_store();

        let shared_id = store.write_block(b"shared content").await.unwrap();
        let orphan_id = store.write_block(b"orphan content").await.unwrap();
        let shared_json =
            format!(r#"{{"block_ids":["{shared_id}"],"size":10,"type":1,"version":1}}"#);

        // Older commit (pruned by history_limit=1) references the block via fileA.
        insert_commit(&db, "c1", "root-c1", 1000).await;
        insert_fs_object(
            &db,
            "root-c1",
            3,
            r#"{"dirents":[{"id":"fileA","mode":33188,"modifier":"u1","mtime":1000,"name":"a.txt","size":10}],"type":3,"version":1}"#,
        )
        .await;
        insert_fs_object(&db, "fileA", 1, &shared_json).await;

        // Retained head commit still references the block via fileB.
        insert_commit(&db, "c2", "root-c2", 3000).await;
        insert_fs_object(
            &db,
            "root-c2",
            3,
            r#"{"dirents":[{"id":"fileB","mode":33188,"modifier":"u1","mtime":1000,"name":"b.txt","size":10}],"type":3,"version":1}"#,
        )
        .await;
        insert_fs_object(&db, "fileB", 1, &shared_json).await;

        let removed = GcManager::garbage_collect(&repos, &store)
            .await
            .expect("gc succeeds");
        // 2 orphaned fs rows (root-c1, fileA) + 1 orphan block.
        assert_eq!(removed, 3);
        // Shared block is still referenced by c2's fileB → must survive.
        assert!(
            store.has_block(&shared_id).await,
            "shared block must survive"
        );
        assert!(
            !store.has_block(&orphan_id).await,
            "orphan block must be deleted"
        );
    }

    /// When the TTL window retains no commits at all (all history expired),
    /// every fs_object of the repo becomes unreachable and is pruned.
    #[tokio::test]
    async fn test_gc_prunes_all_when_no_commits_kept() {
        let db = setup_gc_test_db(0, 1).await; // 1-day TTL
        let repos = crate::repository::Repositories::new(Arc::new(db.clone()));
        let (_dir, store) = temp_block_store();

        // Both commits are older than the 1-day TTL window, so keep is empty.
        let old = chrono::Utc::now().timestamp() - 172_800; // 2 days ago
        insert_commit(&db, "c1", "root-c1", old).await;
        insert_commit(&db, "c2", "root-c2", old).await;
        insert_fs_object(&db, "root-c1", 3, r#"{"dirents":[],"type":3,"version":1}"#).await;
        insert_fs_object(&db, "root-c2", 3, r#"{"dirents":[],"type":3,"version":1}"#).await;

        let removed = GcManager::garbage_collect(&repos, &store)
            .await
            .expect("gc succeeds");
        // Both fs objects and both commits are orphaned and pruned.
        assert_eq!(removed, 2);
        assert_eq!(count_rows(&db, "commits").await, 0);
        assert_eq!(count_rows(&db, "fs_objects").await, 0);
    }
}
