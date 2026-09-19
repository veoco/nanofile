use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

/// Records the container a cached thumbnail was encoded in (`jpeg` or `png`).
///
/// Thumbnails used to be PNG unconditionally; they now follow the pixels
/// (opaque → JPEG, transparent → PNG), so the serving path has to know which
/// container a cache entry holds without decoding it. Rows written before this
/// column existed are all PNG, which is exactly what the default supplies, so
/// they stay servable without a regeneration.
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
    use sea_orm::{ConnectionTrait, Database, DatabaseBackend, DatabaseConnection, Statement};
    use sea_orm_migration::MigratorTrait;

    /// A database migrated up to — but not including — this migration, i.e. the
    /// schema an existing installation is on.
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

    /// Thumbnails cached before this migration were all PNG. The upgrade has to
    /// leave them servable without a regeneration, which is what the column
    /// default has to supply for the rows that already exist.
    #[tokio::test]
    async fn existing_rows_default_to_png() {
        let (_dir, db) = before_this_migration().await;
        assert_eq!(
            column_default(&db).await,
            None,
            "the format column must not exist before this migration"
        );

        // A row written by the old code: no `format` value. Size 48 is one the
        // UI still asks for, so the later legacy-size purge leaves it alone and
        // this test keeps asserting on a surviving row.
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
