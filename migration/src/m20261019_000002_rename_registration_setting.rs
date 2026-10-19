use sea_orm_migration::prelude::*;

/// Rename the registration switch to the name its label uses.
///
/// `auth.enable_invitations` became `auth.enable_registration`: the switch was
/// always "allow self-registration", while invitations are now a separate
/// decision (`auth.require_invitation`). A saved row from an older release must
/// keep its value, or an installation that closed registration would reopen it
/// by upgrading.
///
/// Both keys are **frozen literals on purpose**: this migration moves what an
/// older release wrote, not whatever the catalog lists today.
#[derive(DeriveMigrationName)]
pub struct Migration;

const OLD_KEY: &str = "auth.enable_invitations";
const NEW_KEY: &str = "auth.enable_registration";

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        // A row saved under the new key already wins; the old row is then only
        // removed. `INSERT OR IGNORE` preserves the new value in that case.
        manager
            .get_connection()
            .execute_unprepared(&format!(
                "INSERT OR IGNORE INTO settings (key, value, updated_at, updated_by) \
                 SELECT '{NEW_KEY}', value, updated_at, updated_by FROM settings \
                 WHERE key = '{OLD_KEY}'"
            ))
            .await?;
        manager
            .get_connection()
            .execute_unprepared(&format!("DELETE FROM settings WHERE key = '{OLD_KEY}'"))
            .await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared(&format!(
                "INSERT OR IGNORE INTO settings (key, value, updated_at, updated_by) \
                 SELECT '{OLD_KEY}', value, updated_at, updated_by FROM settings \
                 WHERE key = '{NEW_KEY}'"
            ))
            .await?;
        manager
            .get_connection()
            .execute_unprepared(&format!("DELETE FROM settings WHERE key = '{NEW_KEY}'"))
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
            .position(|name| *name == "m20261019_000002_rename_registration_setting")
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

    /// An old saved value moves to the new key with its provenance intact.
    #[tokio::test]
    async fn moves_the_old_row() {
        let (_dir, db) = before_this_migration().await;
        insert(&db, "auth.enable_invitations", "false", 41).await;
        insert(&db, "server.port", "8082", 1).await;

        crate::Migrator::up(&db, None)
            .await
            .expect("apply this migration");

        assert_eq!(
            rows(&db).await,
            vec![
                (
                    "auth.enable_registration".to_string(),
                    "false".to_string(),
                    41,
                    Some(7)
                ),
                ("server.port".to_string(), "8082".to_string(), 1, Some(7)),
            ]
        );

        let _ = db.close().await;
    }

    /// If both keys exist, the new one is already the operator's choice.
    #[tokio::test]
    async fn the_new_row_wins_when_both_exist() {
        let (_dir, db) = before_this_migration().await;
        insert(&db, "auth.enable_invitations", "false", 11).await;
        insert(&db, "auth.enable_registration", "true", 22).await;

        crate::Migrator::up(&db, None)
            .await
            .expect("apply this migration");

        assert_eq!(
            rows(&db).await,
            vec![(
                "auth.enable_registration".to_string(),
                "true".to_string(),
                22,
                Some(7)
            )]
        );

        let _ = db.close().await;
    }

    /// A database that never had the old row is untouched.
    #[tokio::test]
    async fn a_database_without_the_old_row_is_unchanged() {
        let (_dir, db) = before_this_migration().await;
        insert(&db, "auth.enable_password_reset", "true", 5).await;

        crate::Migrator::up(&db, None)
            .await
            .expect("apply this migration");

        assert_eq!(
            rows(&db).await,
            vec![(
                "auth.enable_password_reset".to_string(),
                "true".to_string(),
                5,
                Some(7)
            )]
        );

        let _ = db.close().await;
    }
}
