use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

/// Drop thumbnail cache rows for sizes the web UI no longer requests.
///
/// The UI now asks for 48 (list icon) and 640 (grid / gallery / drawer); every 256 row
/// is unreachable and pins a PNG on disk. Only rows can be removed here — a migration
/// has no access to the configured thumbnail directory — so the bytes are removed by
/// `service::fs::thumbnail::purge_legacy_cache_files` on first start, marker-guarded.
/// The halves are independent and neither is required to serve a request (a missing
/// entry is regenerated).
///
/// The size literals are **frozen on purpose**: a migration must not change what it
/// deletes when `THUMBNAIL_SIZE_*` is edited later.
#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared("DELETE FROM thumbnails WHERE size NOT IN (48, 640)")
            .await?;
        Ok(())
    }

    /// Nothing to undo: these rows described a cache; a re-requested size is regenerated.
    async fn down(&self, _manager: &SchemaManager) -> Result<(), DbErr> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use sea_orm::{ConnectionTrait, DatabaseBackend, DatabaseConnection, Statement};
    use sea_orm_migration::MigratorTrait;

    /// A database migrated to this migration's own chain position, not `len() - 1` (which
    /// silently becomes "everything including this one" when a migration is appended).
    async fn before_this_migration() -> (tempfile::TempDir, DatabaseConnection) {
        let dir = tempfile::tempdir().expect("tempdir");
        let db = crate::tests_support::connect(&crate::tests_support::file_uri(
            dir.path(),
            "test.db",
        ))
        .await;
        let steps = crate::migration_names()
            .iter()
            .position(|name| *name == "m20260918_000002_purge_legacy_thumbnail_sizes")
            .expect("this migration is registered") as u32;
        crate::Migrator::up(&db, Some(steps))
            .await
            .expect("run prior migrations");
        (dir, db)
    }

    async fn sizes(db: &DatabaseConnection) -> Vec<i64> {
        let rows = db
            .query_all_raw(Statement::from_string(
                DatabaseBackend::Sqlite,
                "SELECT size FROM thumbnails ORDER BY size",
            ))
            .await
            .expect("select sizes");
        rows.into_iter()
            .map(|row| row.try_get::<i64>("", "size").expect("size"))
            .collect()
    }

    /// The current UI's sizes survive, every other size — including 256 — is dropped. Sizes
    /// are compared as a set, so 384/512 from older clients are cleaned up too.
    #[tokio::test]
    async fn drops_every_size_the_ui_no_longer_requests() {
        let (_dir, db) = before_this_migration().await;

        // Rows from the old UI (256) and other callers, with retained sizes mixed in.
        db.execute_unprepared("PRAGMA foreign_keys = OFF")
            .await
            .expect("disable foreign keys for the raw insert");
        for (path, size) in [
            ("/a.png", 48),
            ("/a.png", 256),
            ("/a.png", 384),
            ("/a.png", 512),
            ("/a.png", 640),
            ("/b.png", 256),
        ] {
            db.execute_unprepared(&format!(
                "INSERT INTO thumbnails (repo_id, path, size, format, file_modified_at, created_at) \
                 VALUES ('repo-1', '{path}', {size}, 'png', 1, 1)"
            ))
            .await
            .expect("insert a pre-migration thumbnail row");
        }
        assert_eq!(sizes(&db).await, vec![48, 256, 256, 384, 512, 640]);

        crate::Migrator::up(&db, None)
            .await
            .expect("apply this migration");

        assert_eq!(
            sizes(&db).await,
            vec![48, 640],
            "only the sizes the UI still requests may survive"
        );

        let _ = db.close().await;
    }
}
