use sea_orm_migration::prelude::*;

/// Split the sharing master switch into its two directions.
///
/// `server.share_link_enabled` closed share links **and** upload links. The two
/// halves are now separate settings, `server.share_download_enabled` and
/// `server.share_upload_enabled`, so an operator who hardened an installation by
/// turning the old switch off must not have anonymous uploads reopened by an
/// upgrade. A saved row is copied into **both** new keys (the old value's
/// timestamp and author included), then removed; a new key that already exists
/// wins, because it is the operator's more recent choice.
///
/// Both key names are **frozen literals on purpose**: this migration moves what an
/// older release wrote, not whatever the catalog lists today.
#[derive(DeriveMigrationName)]
pub struct Migration;

const OLD_KEY: &str = "server.share_link_enabled";
const NEW_KEYS: [&str; 2] = [
    "server.share_download_enabled",
    "server.share_upload_enabled",
];

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let conn = manager.get_connection();
        for new_key in NEW_KEYS {
            // `INSERT OR IGNORE` keeps a value saved under the new key: the old
            // master only fills the half nobody has configured yet.
            conn.execute_unprepared(&format!(
                "INSERT OR IGNORE INTO settings (key, value, updated_at, updated_by) \
                 SELECT '{new_key}', value, updated_at, updated_by FROM settings \
                 WHERE key = '{OLD_KEY}'"
            ))
            .await?;
        }
        conn.execute_unprepared(&format!("DELETE FROM settings WHERE key = '{OLD_KEY}'"))
            .await?;
        Ok(())
    }

    /// Best effort, and lossy by nature: one value cannot represent two. The old
    /// master is restored as `false` when either direction was off, so a rollback
    /// never reopens a half the operator had closed.
    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let conn = manager.get_connection();
        conn.execute_unprepared(&format!(
            "INSERT OR IGNORE INTO settings (key, value, updated_at, updated_by) \
             SELECT '{OLD_KEY}', \
                    CASE WHEN SUM(CASE WHEN value = 'false' THEN 1 ELSE 0 END) > 0 \
                         THEN 'false' ELSE 'true' END, \
                    COALESCE(MIN(updated_at), 0), NULL \
             FROM settings WHERE key IN ('{}', '{}') \
             HAVING COUNT(*) > 0",
            NEW_KEYS[0], NEW_KEYS[1]
        ))
        .await?;
        conn.execute_unprepared(&format!(
            "DELETE FROM settings WHERE key IN ('{}', '{}')",
            NEW_KEYS[0], NEW_KEYS[1]
        ))
        .await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use sea_orm::{ConnectionTrait, DatabaseBackend, DatabaseConnection, Statement};
    use sea_orm_migration::MigratorTrait;

    /// A database migrated up to — but not including — this migration.
    async fn before_this_migration() -> (tempfile::TempDir, DatabaseConnection) {
        let dir = tempfile::tempdir().expect("tempdir");
        let db = crate::tests_support::connect(&crate::tests_support::file_uri(
            dir.path(),
            "test.db",
        ))
        .await;
        let steps = crate::migration_names()
            .iter()
            .position(|name| *name == "m20261019_000003_split_share_link_switch")
            .expect("this migration is registered") as u32;
        crate::Migrator::up(&db, Some(steps))
            .await
            .expect("run prior migrations");
        (dir, db)
    }

    async fn insert(db: &DatabaseConnection, key: &str, value: &str, updated_at: i64) {
        db.execute_unprepared(&format!(
            "INSERT INTO settings (key, value, updated_at, updated_by) \
             VALUES ('{key}', '{value}', {updated_at}, 7)"
        ))
        .await
        .expect("insert a pre-migration setting");
    }

    /// key, value, updated_at, updated_by, ordered by key.
    async fn rows(db: &DatabaseConnection) -> Vec<(String, String, i64, Option<i32>)> {
        db.query_all_raw(Statement::from_string(
            DatabaseBackend::Sqlite,
            "SELECT key, value, updated_at, updated_by FROM settings ORDER BY key",
        ))
        .await
        .expect("select settings")
        .into_iter()
        .map(|row| {
            (
                row.try_get::<String>("", "key").expect("key"),
                row.try_get::<String>("", "value").expect("value"),
                row.try_get::<i64>("", "updated_at").expect("updated_at"),
                row.try_get::<Option<i32>>("", "updated_by")
                    .expect("updated_by"),
            )
        })
        .collect()
    }

    /// A saved "off" closes both halves with its provenance intact.
    #[tokio::test]
    async fn a_saved_off_closes_both_directions() {
        let (_dir, db) = before_this_migration().await;
        insert(&db, "server.share_link_enabled", "false", 41).await;
        insert(&db, "server.port", "8082", 1).await;

        crate::Migrator::up(&db, None)
            .await
            .expect("apply this migration");

        assert_eq!(
            rows(&db).await,
            vec![
                (
                    "server.port".to_string(),
                    "8082".to_string(),
                    1,
                    Some(7)
                ),
                (
                    "server.share_download_enabled".to_string(),
                    "false".to_string(),
                    41,
                    Some(7)
                ),
                (
                    "server.share_upload_enabled".to_string(),
                    "false".to_string(),
                    41,
                    Some(7)
                ),
            ]
        );

        let _ = db.close().await;
    }

    /// A direction already saved under its new key wins; the old master only fills
    /// the missing half.
    #[tokio::test]
    async fn an_existing_new_row_wins() {
        let (_dir, db) = before_this_migration().await;
        insert(&db, "server.share_link_enabled", "false", 11).await;
        insert(&db, "server.share_upload_enabled", "true", 22).await;

        crate::Migrator::up(&db, None)
            .await
            .expect("apply this migration");

        assert_eq!(
            rows(&db).await,
            vec![
                (
                    "server.share_download_enabled".to_string(),
                    "false".to_string(),
                    11,
                    Some(7)
                ),
                (
                    "server.share_upload_enabled".to_string(),
                    "true".to_string(),
                    22,
                    Some(7)
                ),
            ]
        );

        let _ = db.close().await;
    }

    /// A database that never had the old row is untouched.
    #[tokio::test]
    async fn a_database_without_the_old_row_is_unchanged() {
        let (_dir, db) = before_this_migration().await;
        insert(&db, "server.file_search_enabled", "false", 5).await;

        crate::Migrator::up(&db, None)
            .await
            .expect("apply this migration");

        assert_eq!(
            rows(&db).await,
            vec![(
                "server.file_search_enabled".to_string(),
                "false".to_string(),
                5,
                Some(7)
            )]
        );

        let _ = db.close().await;
    }
}
