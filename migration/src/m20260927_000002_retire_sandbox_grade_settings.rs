use sea_orm_migration::prelude::*;

/// Drop the two sandbox settings this release retired.
///
/// `sandbox.min_level` was the grade a host had to reach before any parsing
/// feature ran, and `sandbox.windows_container` decided whether a Windows launch
/// should ask for its AppContainer. Neither is a decision any more: the resource
/// bound is the one thing a child must establish and it is read back from the
/// child's own report, while the layers above it — files, the network, process
/// creation, the container — are what the platform can supply. Nothing reads
/// either key, so the rows are invisible to the settings page and to every
/// resolution path; but they are also values an operator cannot see, change or
/// delete, and a row that looks like configuration while doing nothing is worse
/// than no row at all.
///
/// The keys are **frozen literals on purpose**, like `index.sandbox` in
/// `m20260926_000001`: the point is to remove what an older release wrote, not to
/// agree with whatever the catalog happens to list today. A future retirement
/// adds its own key here — and nothing else, because the table is otherwise
/// schemaless by design.
///
/// There is no `down`: the values described a model the new one cannot express,
/// so inventing them back would be a lie.
#[derive(DeriveMigrationName)]
pub struct Migration;

/// Every settings key this release retires.
const RETIRED_KEYS: &[&str] = &["sandbox.min_level", "sandbox.windows_container"];

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
            .position(|name| *name == "m20260927_000002_retire_sandbox_grade_settings")
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

    /// Both retired keys go, whatever they hold, and every live key stays.
    #[tokio::test]
    async fn drops_only_the_retired_keys() {
        let (_dir, db) = before_this_migration().await;

        // The values an older release could have written, including the one a
        // rename had already rewritten: the key goes whatever it holds.
        insert(&db, "sandbox.min_level", "limits").await;
        insert(&db, "sandbox.windows_container", "true").await;
        insert(&db, "sandbox.enabled", "true").await;
        insert(&db, "index.enabled", "true").await;

        crate::Migrator::up(&db, None)
            .await
            .expect("apply this migration");

        assert_eq!(
            keys(&db).await,
            vec!["index.enabled".to_string(), "sandbox.enabled".to_string()]
        );

        let _ = db.close().await;
    }
}
