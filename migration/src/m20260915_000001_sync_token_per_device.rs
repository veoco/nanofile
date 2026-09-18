use sea_orm_migration::prelude::*;

/// Give every device its own repository sync token.
///
/// `sync_tokens` was created with `UNIQUE(repo_id, user_id)`, and issuance
/// reused that single row, so every device of an account shared one token. Peer
/// info is a property of a token, so only one device could ever be recorded on
/// it — and the sync path overwrote `peer_id` on each sync, moving the token
/// from device to device.
///
/// Upstream seafile never had that constraint: `RepoUserToken` is unique on
/// `(repo_id, token)` only, and each token has its own `RepoTokenPeerInfo` row,
/// so one repository has one token *per device*. This migration restores that
/// shape: the per-`(repo, user)` uniqueness goes away, and `(repo, user, peer)`
/// becomes the identity of a token, with `NULL` (or empty) `peer_id` standing
/// for the single unattributed token a device-less caller shares.
///
/// Existing rows keep their token value, so no client has to log in again: they
/// merely gain a device attribution. A token that was minted by a build that
/// never recorded one is attributed to its owner's only known device when that
/// is unambiguous; a multi-device account is left for the first sync to claim,
/// because guessing between two devices would be worse than a temporary
/// "unattributed" entry.
#[derive(DeriveMigrationName)]
pub struct Migration;

/// Attribute unattributed tokens of a single-device account to that device.
///
/// `COUNT(DISTINCT device_id) = 1` is the whole safety property: with two
/// devices there is no evidence of which one minted a token, and a wrong guess
/// would show one device's token under another. Multiple sessions of the same
/// device still count as one.
const BACKFILL_SINGLE_DEVICE: &str = "
UPDATE sync_tokens
   SET peer_id = (
           SELECT t.device_id
             FROM api_tokens t
            WHERE t.user_id = sync_tokens.user_id
              AND t.device_id IS NOT NULL AND t.device_id <> ''
            GROUP BY t.user_id
           HAVING COUNT(DISTINCT t.device_id) = 1
       ),
       peer_name = COALESCE(peer_name, (
           SELECT t.device_name
             FROM api_tokens t
            WHERE t.user_id = sync_tokens.user_id
              AND t.device_id IS NOT NULL AND t.device_id <> ''
            GROUP BY t.user_id
           HAVING COUNT(DISTINCT t.device_id) = 1
       )),
       client_version = COALESCE(client_version, (
           SELECT t.client_version
             FROM api_tokens t
            WHERE t.user_id = sync_tokens.user_id
              AND t.device_id IS NOT NULL AND t.device_id <> ''
            GROUP BY t.user_id
           HAVING COUNT(DISTINCT t.device_id) = 1
       ))
 WHERE (peer_id IS NULL OR peer_id = '')
   AND (
           SELECT COUNT(DISTINCT t.device_id)
             FROM api_tokens t
            WHERE t.user_id = sync_tokens.user_id
              AND t.device_id IS NOT NULL AND t.device_id <> ''
       ) = 1
";

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let db = manager.get_connection();

        // 1. Drop the per-(repo, user) constraint: one token per device, not
        //    one per account.
        db.execute_unprepared("DROP INDEX IF EXISTS idx_sync_tokens_repo_user_unique")
            .await?;

        // 2. Best-effort attribution of rows left without a device.
        db.execute_unprepared(BACKFILL_SINGLE_DEVICE).await?;

        // 3. One token per (repository, user, device). `COALESCE` folds `''`
        //    into `NULL` so a caller whose device id was empty cannot mint a
        //    second unattributed token; SQLite treats NULLs as distinct in a
        //    unique index, which is why the sentinel is needed at all.
        db.execute_unprepared(
            "CREATE UNIQUE INDEX IF NOT EXISTS uidx_sync_tokens_repo_user_peer
                 ON sync_tokens (repo_id, user_id, COALESCE(peer_id, ''))",
        )
        .await?;

        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let db = manager.get_connection();

        db.execute_unprepared("DROP INDEX IF EXISTS uidx_sync_tokens_repo_user_peer")
            .await?;

        // The old constraint can only be restored after collapsing the rows it
        // forbids: keep the oldest token of each (repo, user) group. This loses
        // every other device's token, so a rollback forces those devices to
        // re-acquire one — documented rather than hidden.
        db.execute_unprepared(
            "DELETE FROM sync_tokens WHERE id NOT IN (
                 SELECT MIN(id) FROM sync_tokens GROUP BY repo_id, user_id
             )",
        )
        .await?;

        db.execute_unprepared(
            "CREATE UNIQUE INDEX IF NOT EXISTS idx_sync_tokens_repo_user_unique
                 ON sync_tokens (repo_id, user_id)",
        )
        .await?;

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use sea_orm::{ConnectionTrait, Database, DatabaseBackend, DatabaseConnection, Statement};
    use sea_orm_migration::MigratorTrait;

    /// A database migrated up to — but not including — this migration, so the
    /// old per-`(repo, user)` constraint is still in place.
    ///
    /// The stop point is this migration's own position in the chain, not
    /// `len() - 1`: the latter silently became "everything including this one"
    /// as soon as another migration was appended after it, at which point the
    /// old index these tests assert on no longer existed.
    async fn before_this_migration() -> (tempfile::TempDir, DatabaseConnection) {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("test.db");
        let db = Database::connect(format!("sqlite://{}?mode=rwc", path.display()))
            .await
            .expect("connect sqlite");
        let steps = crate::migration_names()
            .iter()
            .position(|name| *name == "m20260915_000001_sync_token_per_device")
            .expect("this migration is registered") as u32;
        crate::Migrator::up(&db, Some(steps))
            .await
            .expect("run prior migrations");
        (dir, db)
    }

    async fn exec(db: &DatabaseConnection, sql: &str) -> Result<(), sea_orm::DbErr> {
        db.execute_unprepared(sql).await.map(|_| ())
    }

    async fn count_index(db: &DatabaseConnection, name: &str) -> i64 {
        let row = db
            .query_one_raw(Statement::from_sql_and_values(
                DatabaseBackend::Sqlite,
                "SELECT COUNT(*) AS n FROM sqlite_master WHERE type = 'index' AND name = ?",
                [name.into()],
            ))
            .await
            .expect("query sqlite_master")
            .expect("a row");
        row.try_get::<i64>("", "n").expect("count")
    }

    async fn peer_of(db: &DatabaseConnection, repo_id: &str, user_id: i32) -> Option<String> {
        let row = db
            .query_one_raw(Statement::from_sql_and_values(
                DatabaseBackend::Sqlite,
                "SELECT peer_id FROM sync_tokens WHERE repo_id = ? AND user_id = ?",
                [repo_id.into(), user_id.into()],
            ))
            .await
            .expect("query sync_tokens")
            .expect("a row");
        row.try_get::<Option<String>>("", "peer_id").expect("peer")
    }

    /// Seed two accounts and one library: user 1 has one device, user 2 has two.
    async fn seed(db: &DatabaseConnection) {
        exec(
            db,
            "INSERT INTO users (email, password_hash, is_active, created_at)
             VALUES ('one@example.com', 'h', 1, 0), ('two@example.com', 'h', 1, 0)",
        )
        .await
        .expect("seed users");
        exec(
            db,
            "INSERT INTO repos (id, name, owner_id, created_at, updated_at)
             VALUES ('r1', 'lib', 1, 0, 0)",
        )
        .await
        .expect("seed repo");
        exec(
            db,
            "INSERT INTO api_tokens (user_id, token, created_at, device_id, device_name, client_version)
             VALUES (1, 'api-1', 0, 'dev-a', 'Laptop', '3.0.4'),
                    (2, 'api-2', 0, 'dev-b', 'Laptop', '3.0.4'),
                    (2, 'api-3', 0, 'dev-c', 'Phone', '3.0.4')",
        )
        .await
        .expect("seed api tokens");
        exec(
            db,
            "INSERT INTO sync_tokens (repo_id, user_id, token, created_at)
             VALUES ('r1', 1, 'st-1', 0), ('r1', 2, 'st-2', 0)",
        )
        .await
        .expect("seed sync tokens");
    }

    #[tokio::test]
    async fn one_token_per_device_replaces_one_per_account() {
        let (_dir, db) = before_this_migration().await;
        seed(&db).await;

        crate::Migrator::up(&db, None).await.expect("run migration");

        assert_eq!(count_index(&db, "idx_sync_tokens_repo_user_unique").await, 0);
        assert_eq!(count_index(&db, "uidx_sync_tokens_repo_user_peer").await, 1);

        // A second device's token for the same library is allowed…
        exec(
            &db,
            "INSERT INTO sync_tokens (repo_id, user_id, token, created_at, peer_id)
             VALUES ('r1', 2, 'st-3', 0, 'dev-d')",
        )
        .await
        .expect("a second device may hold its own token");

        // …while a duplicate of the same (repo, user, device) is not.
        assert!(
            exec(
                &db,
                "INSERT INTO sync_tokens (repo_id, user_id, token, created_at, peer_id)
                 VALUES ('r1', 2, 'st-4', 0, 'dev-d')",
            )
            .await
            .is_err(),
            "the same device cannot hold two tokens for one library"
        );

        // An empty peer id is the same bucket as NULL, so a device-less caller
        // cannot mint a second unattributed token either.
        exec(
            &db,
            "INSERT INTO sync_tokens (repo_id, user_id, token, created_at, peer_id)
             VALUES ('r1', 1, 'st-5', 0, '')",
        )
        .await
        .expect("one unattributed token");
        assert!(
            exec(
                &db,
                "INSERT INTO sync_tokens (repo_id, user_id, token, created_at, peer_id)
                 VALUES ('r1', 1, 'st-6', 0, NULL)",
            )
            .await
            .is_err(),
            "an empty peer and NULL are one bucket"
        );
    }

    #[tokio::test]
    async fn unattributed_tokens_of_a_single_device_account_are_attributed() {
        let (_dir, db) = before_this_migration().await;
        seed(&db).await;

        crate::Migrator::up(&db, None).await.expect("run migration");

        assert_eq!(
            peer_of(&db, "r1", 1).await.as_deref(),
            Some("dev-a"),
            "the only known device owns the token"
        );
        assert_eq!(
            peer_of(&db, "r1", 2).await,
            None,
            "two devices are ambiguous, so the guess is refused"
        );
    }

    /// The backfill is idempotent: re-running it on an already attributed row
    /// changes nothing, and a row attributed *after* the migration is left
    /// alone.
    #[tokio::test]
    async fn the_backfill_never_reassigns() {
        let (_dir, db) = before_this_migration().await;
        seed(&db).await;
        crate::Migrator::up(&db, None).await.expect("run migration");

        // User 2 gains a device of its own: now the token is unambiguous.
        exec(&db, "DELETE FROM api_tokens WHERE token = 'api-3'")
            .await
            .expect("remove the second device");
        db.execute_unprepared(super::BACKFILL_SINGLE_DEVICE)
            .await
            .expect("re-run the backfill");
        assert_eq!(peer_of(&db, "r1", 2).await.as_deref(), Some("dev-b"));

        // A sync later attaches the token to another device; the backfill must
        // not touch a row that already has an owner.
        exec(
            &db,
            "UPDATE sync_tokens SET peer_id = 'dev-x' WHERE repo_id = 'r1' AND user_id = 2",
        )
        .await
        .expect("attach a device");
        db.execute_unprepared(super::BACKFILL_SINGLE_DEVICE)
            .await
            .expect("re-run the backfill");
        assert_eq!(peer_of(&db, "r1", 2).await.as_deref(), Some("dev-x"));
    }
}
