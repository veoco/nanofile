use sea_orm_migration::prelude::*;

/// Drop settings rows whose key no longer exists in the catalog.
///
/// `index.sandbox` was the four-value policy (`require`/`strict`/`sealed`/
/// `prefer`) that once decided whether a document could be parsed. It is
/// replaced by the `[sandbox]` section: `sandbox.enabled` is the master switch
/// and `sandbox.min_level` is the grade the host must reach. Nothing reads the
/// old key any more, so the row is invisible to the settings page and to every
/// resolution path — but it is also a value an operator cannot see, change or
/// delete, and a row that looks like configuration while doing nothing is worse
/// than no row at all.
///
/// The key is a **frozen literal on purpose**, like the thumbnail sizes in
/// `m20260918_000002`: the point is to remove what an older release wrote, not
/// to agree with whatever the catalog happens to list today. A future retirement
/// adds its own key here — and nothing else, because the table is otherwise
/// schemaless by design.
///
/// There is no `down`: the value was a policy token the new model cannot
/// express, so inventing one back would be a lie.
#[derive(DeriveMigrationName)]
pub struct Migration;

/// Every settings key this release retires.
const RETIRED_KEYS: &[&str] = &["index.sandbox"];

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        for key in RETIRED_KEYS {
            manager
                .get_connection()
                .execute_unprepared(&format!("DELETE FROM settings WHERE key = '{key}'"))
                .await?;
        }
        Ok(())
    }

    async fn down(&self, _manager: &SchemaManager) -> Result<(), DbErr> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use sea_orm::{ConnectionTrait, Database, DatabaseBackend, DatabaseConnection, Statement};
    use sea_orm_migration::MigratorTrait;

    /// A database migrated up to — but not including — this migration.
    async fn before_this_migration() -> (tempfile::TempDir, DatabaseConnection) {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("test.db");
        let db = Database::connect(format!("sqlite://{}?mode=rwc", path.display()))
            .await
            .expect("connect sqlite");
        let steps = crate::migration_names()
            .iter()
            .position(|name| *name == "m20260926_000001_drop_retired_settings_keys")
            .expect("this migration is registered") as u32;
        crate::Migrator::up(&db, Some(steps))
            .await
            .expect("run prior migrations");
        (dir, db)
    }

    async fn keys(db: &DatabaseConnection) -> Vec<String> {
        let rows = db
            .query_all_raw(Statement::from_string(
                DatabaseBackend::Sqlite,
                "SELECT key FROM settings ORDER BY key",
            ))
            .await
            .expect("select keys");
        rows.into_iter()
            .map(|row| row.try_get::<String>("", "key").expect("key"))
            .collect()
    }

    async fn insert(db: &DatabaseConnection, key: &str, value: &str) {
        db.execute_unprepared(&format!(
            "INSERT INTO settings (key, value, updated_at) VALUES ('{key}', '{value}', 0)"
        ))
        .await
        .expect("insert a pre-migration setting");
    }

    /// The retired key goes; every live key stays, whatever it is.
    #[tokio::test]
    async fn drops_only_the_retired_keys() {
        let (_dir, db) = before_this_migration().await;

        insert(&db, "index.sandbox", "strict").await;
        insert(&db, "sandbox.enabled", "true").await;
        insert(&db, "sandbox.min_level", "full").await;
        insert(&db, "index.enabled", "true").await;

        crate::Migrator::up(&db, None)
            .await
            .expect("apply this migration");

        assert_eq!(
            keys(&db).await,
            vec![
                "index.enabled".to_string(),
                "sandbox.enabled".to_string(),
                "sandbox.min_level".to_string(),
            ]
        );

        let _ = db.close().await;
    }
}
