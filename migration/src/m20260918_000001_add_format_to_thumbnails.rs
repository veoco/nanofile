use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

/// Records the thumbnail's container (`jpeg` or `png`). Thumbnails used to be PNG
/// unconditionally and now follow the pixels, so serving has to know the container
/// without decoding; older rows are all PNG, exactly what the default supplies.
#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(Thumbnails::Table)
                    .add_column(
                        ColumnDef::new(Thumbnails::Format)
                            .string_len(16)
                            .not_null()
                            .default("png"),
                    )
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(Thumbnails::Table)
                    .drop_column(Thumbnails::Format)
                    .to_owned(),
            )
            .await
    }
}

#[derive(Iden)]
enum Thumbnails {
    Table,
    Format,
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
            .position(|name| *name == "m20260918_000001_add_format_to_thumbnails")
            .expect("this migration is registered") as u32;
        crate::Migrator::up(&db, Some(steps))
            .await
            .expect("run prior migrations");
        (dir, db)
    }

    async fn column_default(db: &DatabaseConnection) -> Option<(String, i64, Option<String>)> {
        let rows = db
            .query_all_raw(Statement::from_string(
                DatabaseBackend::Sqlite,
                "SELECT name, \"notnull\", dflt_value FROM pragma_table_info('thumbnails')",
            ))
            .await
            .expect("pragma table_info");
        rows.into_iter()
            .map(|row| {
                (
                    row.try_get::<String>("", "name").expect("column name"),
                    row.try_get::<i64>("", "notnull").expect("notnull flag"),
                    row.try_get::<Option<String>>("", "dflt_value")
                        .expect("default value"),
                )
            })
            .find(|(name, _, _)| name == "format")
    }

    /// Cached rows predate the column and are all PNG; the default must keep them servable
    /// without regeneration.
    #[tokio::test]
    async fn existing_rows_default_to_png() {
        let (_dir, db) = before_this_migration().await;
        assert_eq!(
            column_default(&db).await,
            None,
            "the format column must not exist before this migration"
        );

        // A row from the old code: no `format`. Size 48 survives the later legacy-size purge, so
        // this test keeps asserting on a live row.
        db.execute_unprepared("PRAGMA foreign_keys = OFF")
            .await
            .expect("disable foreign keys for the raw insert");
        db.execute_unprepared(
            "INSERT INTO thumbnails (repo_id, path, size, file_modified_at, created_at) \
             VALUES ('repo-1', '/photo.png', 48, 1, 1)",
        )
        .await
        .expect("insert a pre-migration thumbnail row");

        crate::Migrator::up(&db, None)
            .await
            .expect("apply this migration");

        let (_, not_null, default) = column_default(&db)
            .await
            .expect("format column exists after the upgrade");
        assert_eq!(not_null, 1, "every cache entry has a container");
        // SQLite echoes the literal it stored, quotes included.
        assert_eq!(
            default.as_deref().map(|d| d.trim_matches('\'')),
            Some("png")
        );

        let row = db
            .query_one_raw(Statement::from_string(
                DatabaseBackend::Sqlite,
                "SELECT format FROM thumbnails WHERE path = '/photo.png'",
            ))
            .await
            .expect("read the pre-migration row")
            .expect("the pre-migration row survived");
        assert_eq!(
            row.try_get::<String>("", "format").expect("format"),
            "png",
            "a row cached before the column existed must read back as PNG"
        );

        let _ = db.close().await;
    }
}
