use sea_orm_migration::prelude::*;

/// Automatic "people" tags and the bookkeeping they need.
///
/// `repo_tags` gains a `source`/`label_key` pair so an automatic tag can be
/// distinguished from a hand-made one and re-found after a rename. The scan table
/// is per image (it also records "no face", so the backfill does not re-read every
/// photo). Exclusions remember that a user removed one automatic tag from one
/// file; the next scan must not put it back.
#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        // SQLite's ALTER TABLE takes one action at a time.
        manager
            .alter_table(
                Table::alter()
                    .table(RepoTags::Table)
                    .add_column(
                        ColumnDef::new(RepoTags::Source)
                            .string_len(16)
                            .not_null()
                            .default("manual"),
                    )
                    .to_owned(),
            )
            .await?;
        manager
            .alter_table(
                Table::alter()
                    .table(RepoTags::Table)
                    .add_column(ColumnDef::new(RepoTags::LabelKey).string_len(64).null())
                    .to_owned(),
            )
            .await?;
        manager
            .create_index(
                Index::create()
                    .name("idx_repo_tags_repo_label")
                    .table(RepoTags::Table)
                    .col(RepoTags::RepoId)
                    .col(RepoTags::LabelKey)
                    .to_owned(),
            )
            .await?;

        manager
            .create_table(
                Table::create()
                    .table(FaceTagScans::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(FaceTagScans::Id)
                            .integer()
                            .not_null()
                            .auto_increment()
                            .primary_key(),
                    )
                    .col(ColumnDef::new(FaceTagScans::RepoId).char_len(36).not_null())
                    .col(ColumnDef::new(FaceTagScans::FilePath).text().not_null())
                    .col(ColumnDef::new(FaceTagScans::FsId).char_len(40).not_null())
                    .col(
                        ColumnDef::new(FaceTagScans::ModelVersion)
                            .integer()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(FaceTagScans::Status)
                            .string_len(16)
                            .not_null(),
                    )
                    .col(ColumnDef::new(FaceTagScans::Reason).text().null())
                    .col(
                        ColumnDef::new(FaceTagScans::FaceCount)
                            .integer()
                            .not_null()
                            .default(0),
                    )
                    .col(ColumnDef::new(FaceTagScans::MaxScore).double().null())
                    .col(
                        ColumnDef::new(FaceTagScans::AttemptedAt)
                            .big_integer()
                            .not_null(),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_face_tag_scans_repo")
                            .from(FaceTagScans::Table, FaceTagScans::RepoId)
                            .to(Repos::Table, Repos::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .to_owned(),
            )
            .await?;
        manager
            .create_index(
                Index::create()
                    .name("idx_face_tag_scans_repo_path")
                    .table(FaceTagScans::Table)
                    .col(FaceTagScans::RepoId)
                    .col(FaceTagScans::FilePath)
                    .unique()
                    .to_owned(),
            )
            .await?;
        manager
            .create_index(
                Index::create()
                    .name("idx_face_tag_scans_status")
                    .table(FaceTagScans::Table)
                    .col(FaceTagScans::RepoId)
                    .col(FaceTagScans::Status)
                    .col(FaceTagScans::AttemptedAt)
                    .to_owned(),
            )
            .await?;

        manager
            .create_table(
                Table::create()
                    .table(FaceTagExclusions::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(FaceTagExclusions::Id)
                            .integer()
                            .not_null()
                            .auto_increment()
                            .primary_key(),
                    )
                    .col(
                        ColumnDef::new(FaceTagExclusions::RepoId)
                            .char_len(36)
                            .not_null(),
                    )
                    .col(ColumnDef::new(FaceTagExclusions::FilePath).text().not_null())
                    .col(
                        ColumnDef::new(FaceTagExclusions::LabelKey)
                            .string_len(64)
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(FaceTagExclusions::CreatedAt)
                            .big_integer()
                            .not_null(),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_face_tag_exclusions_repo")
                            .from(FaceTagExclusions::Table, FaceTagExclusions::RepoId)
                            .to(Repos::Table, Repos::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .to_owned(),
            )
            .await?;
        manager
            .create_index(
                Index::create()
                    .name("idx_face_tag_exclusions_repo_path_key")
                    .table(FaceTagExclusions::Table)
                    .col(FaceTagExclusions::RepoId)
                    .col(FaceTagExclusions::FilePath)
                    .col(FaceTagExclusions::LabelKey)
                    .unique()
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(
                Table::drop()
                    .table(FaceTagExclusions::Table)
                    .if_exists()
                    .to_owned(),
            )
            .await?;
        manager
            .drop_table(Table::drop().table(FaceTagScans::Table).if_exists().to_owned())
            .await?;
        manager
            .drop_index(
                Index::drop()
                    .name("idx_repo_tags_repo_label")
                    .table(RepoTags::Table)
                    .to_owned(),
            )
            .await?;
        manager
            .alter_table(
                Table::alter()
                    .table(RepoTags::Table)
                    .drop_column(RepoTags::LabelKey)
                    .to_owned(),
            )
            .await?;
        manager
            .alter_table(
                Table::alter()
                    .table(RepoTags::Table)
                    .drop_column(RepoTags::Source)
                    .to_owned(),
            )
            .await
    }
}

#[derive(Iden)]
enum RepoTags {
    Table,
    RepoId,
    Source,
    LabelKey,
}

#[derive(Iden)]
enum FaceTagScans {
    Table,
    Id,
    RepoId,
    FilePath,
    FsId,
    ModelVersion,
    Status,
    Reason,
    FaceCount,
    MaxScore,
    AttemptedAt,
}

#[derive(Iden)]
enum FaceTagExclusions {
    Table,
    Id,
    RepoId,
    FilePath,
    LabelKey,
    CreatedAt,
}

#[derive(Iden)]
enum Repos {
    Table,
    Id,
}

#[cfg(test)]
mod tests {
    use sea_orm::{ConnectionTrait, DatabaseBackend, DatabaseConnection, Statement};
    use sea_orm_migration::MigratorTrait;

    async fn before_this_migration() -> (tempfile::TempDir, DatabaseConnection) {
        let dir = tempfile::tempdir().expect("tempdir");
        let db = crate::tests_support::connect(&crate::tests_support::file_uri(
            dir.path(),
            "test.db",
        ))
        .await;
        let steps = crate::migration_names()
            .iter()
            .position(|name| *name == "m20261102_000001_create_face_tags")
            .expect("registered") as u32;
        crate::Migrator::up(&db, Some(steps))
            .await
            .expect("prior migrations");
        (dir, db)
    }

    #[tokio::test]
    async fn existing_tags_are_manual_and_new_tables_exist() {
        let (_dir, db) = before_this_migration().await;
        db.execute_unprepared("PRAGMA foreign_keys = OFF")
            .await
            .expect("disable foreign keys for the raw insert");
        db.execute_unprepared(
            "INSERT INTO repo_tags (repo_id, name, color, created_at) \
             VALUES ('repo-1', 'old', '#123456', 1)",
        )
        .await
        .expect("pre-migration tag");
        crate::Migrator::up(&db, None).await.expect("apply");

        let row = db
            .query_one_raw(Statement::from_string(
                DatabaseBackend::Sqlite,
                "SELECT source, label_key FROM repo_tags WHERE name = 'old'",
            ))
            .await
            .expect("query")
            .expect("row");
        assert_eq!(row.try_get::<String>("", "source").unwrap(), "manual");
        assert!(
            row.try_get::<Option<String>>("", "label_key")
                .unwrap()
                .is_none()
        );
        for table in ["face_tag_scans", "face_tag_exclusions"] {
            let found = db
                .query_one_raw(Statement::from_string(
                    DatabaseBackend::Sqlite,
                    format!(
                        "SELECT name FROM sqlite_master WHERE type = 'table' AND name = '{table}'"
                    ),
                ))
                .await
                .expect("query");
            assert!(found.is_some(), "{table} exists");
        }
        let _ = db.close().await;
    }
}
