use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

/// Drop thumbnail cache rows for sizes the web UI no longer requests.
///
/// The UI used to ask for `size=256` tiles; it now asks for 48 (list icon) and
/// 640 (grid / gallery / drawer). Every 256 row is therefore unreachable, and
/// each one pins a PNG on disk — ~45-105 KB per photo depending on content, so
/// tens of GB across a large library.
///
/// Only the rows can be removed from here: a schema migration has no access to
/// the configured thumbnail directory. The bytes are removed by
/// `service::fs::thumbnail::purge_legacy_cache_files`, which runs once on the
/// first start of the new binary and is marker-guarded. The two halves are
/// deliberately independent: whichever runs first leaves the cache in a
/// consistent state, and neither is required for the thumbnail endpoint to
/// serve a request (a missing entry is simply regenerated).
///
/// The size literals are **frozen on purpose**. They were the UI's sizes when
/// this migration shipped, and a migration must not change what it deletes when
/// `thumbnail_util::THUMBNAIL_SIZE_*` is edited later — an installation that
/// upgrades from an old release still has to lose its 256 rows even if a future
/// UI asks for 800.
#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared("DELETE FROM thumbnails WHERE size NOT IN (48, 640)")
            .await?;
        Ok(())
    }

    /// Nothing to undo: these rows described a cache, and the files they
    /// pointed at are gone. A re-requested size is regenerated on demand.
    async fn down(&self, _manager: &SchemaManager) -> Result<(), DbErr> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use sea_orm::{ConnectionTrait, Database, DatabaseBackend, DatabaseConnection, Statement};
    use sea_orm_migration::MigratorTrait;

    /// A database migrated up to — but not including — this migration.
    ///
    /// The stop point is this migration's own position in the chain, not
    /// `len() - 1`: the latter silently becomes "everything including this one"
    /// as soon as another migration is appended after it.
    async fn before_this_migration() -> (tempfile::TempDir, DatabaseConnection) {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("test.db");
        let db = Database::connect(format!("sqlite://{}?mode=rwc", path.display()))
            .await
            .expect("connect sqlite");
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

    /// The rows the current UI still asks for survive; every other size — the
    /// old 256 among them — is dropped. Sizes are compared as a set, so an
    /// installation that also cached 384/512 (older clients, manual probing)
    /// is cleaned up by the same statement.
    #[tokio::test]
    async fn drops_every_size_the_ui_no_longer_requests() {
        let (_dir, db) = before_this_migration().await;

        // Rows written by the old UI (256) and by other callers of the same
        // endpoint, with the retained sizes mixed in.
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
