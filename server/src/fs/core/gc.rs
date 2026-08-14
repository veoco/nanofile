use crate::repository::Repositories;
use base::common::{FsDirData, S_IFDIR, SEAF_METADATA_TYPE_DIR};
use base::error::AppError;
use infra::entity::repo;

const SECS_PER_DAY: i64 = 86_400;

pub struct GcManager;

impl GcManager {
    /// Garbage-collect every repo that has history retention limits set.
    ///
    /// For each repo with `history_limit > 0` and/or `history_ttl_days > 0`,
    /// keeps the newest `history_limit` commits plus any commit within the
    /// last `history_ttl_days` days, then deletes unreachable fs objects and
    /// the pruned commit rows. Repos with both settings at 0 are unlimited and
    /// are left untouched.
    pub async fn garbage_collect(repos: &Repositories) -> Result<u64, AppError> {
        let now = chrono::Utc::now().timestamp();
        let all_repos = repos.repo.find_all().await?;

        let mut removed = 0u64;
        for repo_model in &all_repos {
            removed += Self::prune_repo(repos, repo_model, now).await?;
        }
        Ok(removed)
    }

    /// Prune one repo's history according to its retention settings.
    async fn prune_repo(
        repos: &Repositories,
        repo_model: &repo::Model,
        now: i64,
    ) -> Result<u64, AppError> {
        if repo_model.history_limit == 0 && repo_model.history_ttl_days == 0 {
            return Ok(0);
        }

        let commits = repos
            .commit
            .find_by_repo_id_ordered_by_ctime_desc(&repo_model.id)
            .await?;
        if commits.is_empty() {
            return Ok(0);
        }

        // Compute the set of commits to keep. The list is newest-first, so the
        // head commit is index 0 and is always retained.
        let mut keep: std::collections::HashSet<i64> = std::collections::HashSet::new();
        if repo_model.history_limit > 0 {
            for c in commits.iter().take(repo_model.history_limit as usize) {
                keep.insert(c.id);
            }
        }
        if repo_model.history_ttl_days > 0 {
            let cutoff = now - i64::from(repo_model.history_ttl_days) * SECS_PER_DAY;
            for c in &commits {
                if c.ctime >= cutoff {
                    keep.insert(c.id);
                }
            }
        }

        if keep.len() == commits.len() {
            return Ok(0);
        }

        // Collect fs ids reachable from the kept commits' roots.
        let mut active_fs_ids = std::collections::HashSet::new();
        for c in &commits {
            if keep.contains(&c.id) {
                Self::collect_fs_ids(repos, &repo_model.id, &c.root_id, &mut active_fs_ids).await?;
            }
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

    /// In-memory SQLite with the minimal schema + a seeded repo/commits/
    /// fs_objects for GC.
    async fn setup_gc_test_db(
        history_limit: i32,
        history_ttl_days: i32,
    ) -> sea_orm::DatabaseConnection {
        let db = Database::connect("sqlite::memory:").await.unwrap();

        db.execute(Statement::from_string(
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

        db.execute(Statement::from_string(
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
        db.execute(Statement::from_string(
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
        db.execute(Statement::from_string(
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
            .query_one(Statement::from_string(
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
            r#"{"block_ids":["aaaa"],"size":10,"obj_type":1,"version":1}"#,
        )
        .await;
        insert_fs_object(&db, "root-c3", 3, r#"{"dirents":[{"id":"fileB","mode":33188,"modifier":"u1","mtime":1000,"name":"b.txt","size":20}],"type":3,"version":1}"#).await;
        insert_fs_object(
            &db,
            "fileB",
            1,
            r#"{"block_ids":["bbbb"],"size":20,"obj_type":1,"version":1}"#,
        )
        .await;

        let removed = GcManager::garbage_collect(&repos)
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

        insert_commit(&db, "c1", "root-c1", 3000).await;
        insert_commit(&db, "c2", "root-c2", 1000).await;
        insert_fs_object(&db, "root-c1", 3, r#"{"dirents":[],"type":3,"version":1}"#).await;

        let removed = GcManager::garbage_collect(&repos)
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
            r#"{"block_ids":["aaaa"],"size":10,"obj_type":1,"version":1}"#,
        )
        .await;
        insert_fs_object(&db, "root-old", 3, r#"{"dirents":[],"type":3,"version":1}"#).await;

        let removed = GcManager::garbage_collect(&repos)
            .await
            .expect("gc succeeds");
        // root-old is orphaned (only reachable from the pruned commit).
        assert_eq!(removed, 1);
        assert_eq!(count_rows(&db, "commits").await, 1);
        assert_eq!(count_rows(&db, "fs_objects").await, 2);
    }
}
