use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

/// Drop the run history of the two passes that used to fire every 30 seconds.
///
/// The index committer and the outbox drainer were 30-second jobs and every tick was
/// recorded, ~5,760 rows a day, so the run list's newest rows were always those two.
/// Both are gone (a debounced write and a woken service), so stored tick rows are the
/// only thing between an upgrade and a run list that says something.
///
/// The slug literals are **frozen on purpose**, like the thumbnail sizes: an old
/// installation still has to lose these rows. Only finished rows are touched — an
/// unfinished one is somebody's pending work that the recovery pass reads, and closing
/// it as interrupted is the next start's job. Hence no `down`: inventing the rows back
/// would be a lie.
#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared(
                "DELETE FROM job_runs \
                 WHERE finished_at IS NOT NULL \
                   AND kind IN ('index-commit', 'mail-delivery')",
            )
            .await?;
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
            .position(|name| *name == "m20260925_000001_purge_tick_run_history")
            .expect("this migration is registered") as u32;
        crate::Migrator::up(&db, Some(steps))
            .await
            .expect("run prior migrations");
        (dir, db)
    }

    async fn ids(db: &DatabaseConnection) -> Vec<String> {
        let rows = db
            .query_all_raw(Statement::from_string(
                DatabaseBackend::Sqlite,
                "SELECT id FROM job_runs ORDER BY id",
            ))
            .await
            .expect("select ids");
        rows.into_iter()
            .map(|row| row.try_get::<String>("", "id").expect("id"))
            .collect()
    }

    async fn insert(db: &DatabaseConnection, id: &str, kind: &str, finished: Option<i64>) {
        let finished = finished
            .map(|ts| ts.to_string())
            .unwrap_or_else(|| "NULL".to_string());
        db.execute_unprepared(&format!(
            "INSERT INTO job_runs (id, kind, phase, summary, attempt, created_at, finished_at) \
             VALUES ('{id}', '{kind}', 'succeeded', 'did work', 1, 0, {finished})"
        ))
        .await
        .expect("insert a pre-migration run");
    }

    /// Tick histories go; a requested run and an unfinished row both stay.
    #[tokio::test]
    async fn drops_the_recorded_ticks_and_nothing_else() {
        let (_dir, db) = before_this_migration().await;

        insert(&db, "tick-index", "index-commit", Some(1)).await;
        insert(&db, "tick-mail", "mail-delivery", Some(2)).await;
        insert(&db, "a-reindex", "reindex", Some(3)).await;
        insert(&db, "a-gc", "gc", Some(4)).await;
        insert(&db, "crashed-tick", "index-commit", None).await;

        assert_eq!(ids(&db).await.len(), 5);

        crate::Migrator::up(&db, None)
            .await
            .expect("apply this migration");

        assert_eq!(
            ids(&db).await,
            vec![
                "a-gc".to_string(),
                "a-reindex".to_string(),
                // Unfinished work is not history: recovery has to see it.
                "crashed-tick".to_string(),
            ]
        );

        let _ = db.close().await;
    }
}
