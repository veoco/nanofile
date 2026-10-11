use sea_orm_migration::prelude::*;

/// Rewrite the two `sandbox.min_level` values this release renamed.
///
/// The grade vocabulary changed with the model it describes. `none` used to mean
/// "the resource bound and nothing else" and is now called `limits`, and
/// `partial` (the shipped default) keeps its name but now reads as "the resource
/// bound and the files layer" rather than as a confinement grade that the files
/// layer was a necessary condition of. Both are rewritten here so a stored row
/// holds the name the catalog lists and the settings page renders.
///
/// The values are **frozen literals on purpose**, like the retired keys in
/// `m20260926_000001`: the point is to rewrite what an older release wrote, not
/// to agree with whatever the catalog happens to accept today. `Level::parse`
/// keeps reading the old names anyway, so a `config.toml` or environment
/// variable — the two layers this cannot reach — keeps working. What this
/// migration buys is a database row that agrees with the page it is shown on.
///
/// There is no `down`: the rename is the model, and writing the old names back
/// would produce exactly the rows this migration exists to remove.
#[derive(DeriveMigrationName)]
pub struct Migration;

/// One old value and the name that replaced it.
const RENAMES: &[(&str, &str)] = &[("none", "limits")];

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        for (old, new) in RENAMES {
            manager
                .get_connection()
                .execute_unprepared(&format!(
                    "UPDATE settings SET value = '{new}' \
                     WHERE key = 'sandbox.min_level' AND value = '{old}'"
                ))
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
            .position(|name| *name == "m20260927_000001_rename_sandbox_min_level_values")
            .expect("this migration is registered") as u32;
        crate::Migrator::up(&db, Some(steps))
            .await
            .expect("run prior migrations");
        (dir, db)
    }

    async fn value(db: &DatabaseConnection, key: &str) -> Option<String> {
        let rows = db
            .query_all_raw(Statement::from_string(
                DatabaseBackend::Sqlite,
                format!("SELECT value FROM settings WHERE key = '{key}'"),
            ))
            .await
            .expect("select value");
        rows.into_iter()
            .next()
            .map(|row| row.try_get::<String>("", "value").expect("value"))
    }

    async fn insert(db: &DatabaseConnection, key: &str, value: &str) {
        db.execute_unprepared(&format!(
            "INSERT INTO settings (key, value, updated_at) VALUES ('{key}', '{value}', 0)"
        ))
        .await
        .expect("insert a pre-migration setting");
    }

    /// The old name becomes the current one; everything else is left alone.
    #[tokio::test]
    async fn renames_only_the_old_value() {
        let (_dir, db) = before_this_migration().await;

        insert(&db, "sandbox.min_level", "none").await;
        insert(&db, "sandbox.enabled", "true").await;

        crate::Migrator::up(&db, None)
            .await
            .expect("apply this migration");

        assert_eq!(
            value(&db, "sandbox.min_level").await.as_deref(),
            Some("limits")
        );
        assert_eq!(
            value(&db, "sandbox.enabled").await.as_deref(),
            Some("true"),
            "every other key is untouched"
        );

        let _ = db.close().await;
    }

    /// A value the rename does not name — `full`, the other old name, anything
    /// an operator typed — is left exactly as it was found.
    #[tokio::test]
    async fn leaves_every_other_value_alone() {
        let (_dir, db) = before_this_migration().await;
        insert(&db, "sandbox.min_level", "full").await;

        crate::Migrator::up(&db, None)
            .await
            .expect("apply this migration");

        assert_eq!(value(&db, "sandbox.min_level").await.as_deref(), Some("full"));

        let _ = db.close().await;
    }
}
