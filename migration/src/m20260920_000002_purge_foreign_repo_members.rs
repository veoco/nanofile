use sea_orm_migration::prelude::*;

/// Withdraw every library share by purging non-owner memberships.
///
/// nanofile no longer offers user-to-user library sharing, but `repo_members`
/// stays: the permission predicates in `server/src/domain/permission.rs` and
/// every listing read it, and library creation writes the owner's row into it.
/// The rows a previous share created would therefore keep granting access even
/// though no endpoint can undo them any more — so this migration deletes every
/// row whose `user_id` is not its library's `owner_id`.
///
/// The purged users' repository sync tokens go with them. Handlers re-derive
/// the caller's permission on every `/seafhttp/` request, so a surviving token
/// would be refused anyway; deleting it is defence in depth, and it is what the
/// removed unshare operation used to do.
///
/// `down()` is a deliberate no-op: the deleted rows are evidence of a feature
/// that no longer exists, and there is nothing to reconstruct them from.
#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let db = manager.get_connection();

        // Credentials first: a token is what makes the row below reachable
        // without a re-check, so it must not outlive the membership.
        db.execute_unprepared(
            "DELETE FROM sync_tokens
              WHERE NOT EXISTS (
                  SELECT 1 FROM repos r
                   WHERE r.id = sync_tokens.repo_id
                     AND r.owner_id = sync_tokens.user_id
              )",
        )
        .await?;

        db.execute_unprepared(
            "DELETE FROM repo_members
              WHERE NOT EXISTS (
                  SELECT 1 FROM repos r
                   WHERE r.id = repo_members.repo_id
                     AND r.owner_id = repo_members.user_id
              )",
        )
        .await?;

        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        // Irreversible on purpose: see the type-level documentation.
        let _ = manager;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use sea_orm::{ConnectionTrait, Database, DatabaseConnection, Statement};
    use sea_orm_migration::MigratorTrait;

    /// A database migrated up to — but not including — this migration, so the
    /// foreign memberships are still present to be purged.
    async fn before_this_migration() -> (tempfile::TempDir, DatabaseConnection) {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("test.db");
        let db = Database::connect(format!("sqlite://{}?mode=rwc", path.display()))
            .await
            .expect("connect sqlite");
        let steps = crate::migration_names()
            .iter()
            .position(|name| *name == "m20260920_000002_purge_foreign_repo_members")
            .expect("this migration is registered") as u32;
        crate::Migrator::up(&db, Some(steps))
            .await
            .expect("run prior migrations");
        (dir, db)
    }

    async fn exec(db: &DatabaseConnection, sql: &str) {
        db.execute_unprepared(sql).await.expect("seed/exec");
    }

    /// Two accounts, one library owned by user 1. User 1 holds the owner
    /// membership row; user 2 holds a share, and both hold a sync token.
    /// A second library has no membership row at all.
    async fn seed(db: &DatabaseConnection) {
        exec(
            db,
            "INSERT INTO users (email, password_hash, is_active, created_at)
             VALUES ('owner@example.com', 'h', 1, 0), ('member@example.com', 'h', 1, 0)",
        )
        .await;
        exec(
            db,
            "INSERT INTO repos (id, name, owner_id, created_at, updated_at)
             VALUES ('r1', 'shared', 1, 0, 0), ('r2', 'unrelated', 1, 0, 0)",
        )
        .await;
        exec(
            db,
            "INSERT INTO repo_members (repo_id, user_id, permission, created_at)
             VALUES ('r1', 1, 'rw', 0), ('r1', 2, 'r', 0)",
        )
        .await;
        exec(
            db,
            "INSERT INTO sync_tokens (repo_id, user_id, token, created_at)
             VALUES ('r1', 1, 'st-owner', 0), ('r1', 2, 'st-member', 0)",
        )
        .await;
    }

    async fn count_where(db: &DatabaseConnection, sql: &str) -> i64 {
        db.query_one_raw(Statement::from_string(
            sea_orm::DatabaseBackend::Sqlite,
            sql,
        ))
        .await
        .expect("count")
        .expect("a row")
        .try_get::<i64>("", "n")
        .expect("n")
    }

    #[tokio::test]
    async fn purges_foreign_memberships_and_their_sync_tokens() {
        let (_dir, db) = before_this_migration().await;
        seed(&db).await;

        crate::Migrator::up(&db, None).await.expect("run migration");

        // The owner's row survives; the share does not.
        assert_eq!(
            count_where(
                &db,
                "SELECT COUNT(*) AS n FROM repo_members WHERE user_id = 1"
            )
            .await,
            1
        );
        assert_eq!(
            count_where(
                &db,
                "SELECT COUNT(*) AS n FROM repo_members WHERE user_id = 2"
            )
            .await,
            0
        );
        // …and neither does the share's sync token.
        assert_eq!(
            count_where(
                &db,
                "SELECT COUNT(*) AS n FROM sync_tokens WHERE user_id = 2"
            )
            .await,
            0
        );
        // The owner keeps the library's token.
        assert_eq!(
            count_where(
                &db,
                "SELECT COUNT(*) AS n FROM sync_tokens WHERE user_id = 1"
            )
            .await,
            1
        );
    }

    #[tokio::test]
    async fn leaves_a_repo_without_membership_rows_alone() {
        let (_dir, db) = before_this_migration().await;
        seed(&db).await;

        crate::Migrator::up(&db, None).await.expect("run migration");

        assert_eq!(
            count_where(
                &db,
                "SELECT COUNT(*) AS n FROM repos WHERE id = 'r2'"
            )
            .await,
            1
        );
    }
}
