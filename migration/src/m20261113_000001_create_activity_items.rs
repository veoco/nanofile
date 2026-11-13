use sea_orm_migration::prelude::*;

/// Split activity details out of the `activities.detail` JSON:
///
/// * `activities.item_count` carries the exact number of objects in the
///   operation, even when only a bounded slice of details is returned.
/// * `activities.group_kind` separates a single Web call (which the 5-minute
///   session window may still aggregate) from a sync commit group, so a Web
///   upload never appends itself to a commit's summary.
/// * `activity_items` stores one row per object, so a batch from one upload no
///   longer needs a timestamp window to decide what belongs together and the
///   summary row can stay small.
///
/// Rows written before this migration keep `item_count = 0` and their JSON
/// detail; readers fall back to it, so no data is rewritten.
#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(Activities::Table)
                    .add_column(
                        ColumnDef::new(Activities::ItemCount)
                            .integer()
                            .not_null()
                            .default(0),
                    )
                    .to_owned(),
            )
            .await?;
        manager
            .alter_table(
                Table::alter()
                    .table(Activities::Table)
                    .add_column(
                        ColumnDef::new(Activities::GroupKind)
                            .string_len(16)
                            .not_null()
                            .default("session"),
                    )
                    .to_owned(),
            )
            .await?;

        manager
            .create_table(
                Table::create()
                    .table(ActivityItems::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(ActivityItems::Id)
                            .integer()
                            .not_null()
                            .auto_increment()
                            .primary_key(),
                    )
                    .col(ColumnDef::new(ActivityItems::ActivityId).integer().not_null())
                    .col(ColumnDef::new(ActivityItems::Path).text().not_null())
                    .col(ColumnDef::new(ActivityItems::OldPath).text().null())
                    .col(ColumnDef::new(ActivityItems::Size).big_integer().null())
                    .col(ColumnDef::new(ActivityItems::ObjId).char_len(40).null())
                    .col(
                        ColumnDef::new(ActivityItems::Merge)
                            .boolean()
                            .not_null()
                            .default(false),
                    )
                    .col(
                        ColumnDef::new(ActivityItems::MergeKept)
                            .boolean()
                            .not_null()
                            .default(false),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_activity_items_activity_id")
                            .from(ActivityItems::Table, ActivityItems::ActivityId)
                            .to(Activities::Table, Activities::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .name("idx_activity_items_activity")
                    .table(ActivityItems::Table)
                    .col(ActivityItems::ActivityId)
                    .col(ActivityItems::Id)
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_index(
                Index::drop()
                    .name("idx_activity_items_activity")
                    .table(ActivityItems::Table)
                    .to_owned(),
            )
            .await?;
        manager
            .drop_table(
                Table::drop()
                    .table(ActivityItems::Table)
                    .if_exists()
                    .to_owned(),
            )
            .await?;
        manager
            .alter_table(
                Table::alter()
                    .table(Activities::Table)
                    .drop_column(Activities::GroupKind)
                    .to_owned(),
            )
            .await?;
        manager
            .alter_table(
                Table::alter()
                    .table(Activities::Table)
                    .drop_column(Activities::ItemCount)
                    .to_owned(),
            )
            .await
    }
}

#[derive(Iden)]
enum Activities {
    Table,
    Id,
    ItemCount,
    GroupKind,
}

#[derive(Iden)]
enum ActivityItems {
    Table,
    Id,
    ActivityId,
    Path,
    OldPath,
    Size,
    ObjId,
    Merge,
    MergeKept,
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
            .position(|name| *name == "m20261113_000001_create_activity_items")
            .expect("registered") as u32;
        crate::Migrator::up(&db, Some(steps))
            .await
            .expect("prior migrations");
        (dir, db)
    }

    #[tokio::test]
    async fn existing_activity_defaults_to_zero_and_items_table_exists() {
        let (_dir, db) = before_this_migration().await;
        db.execute_unprepared("PRAGMA foreign_keys = OFF")
            .await
            .expect("disable foreign keys for the raw insert");
        db.execute_unprepared(
            "INSERT INTO activities \
             (repo_id, commit_id, op_type, obj_type, path, user_id, created_at, detail) \
             VALUES ('repo-1', 'commit-1', 'create', 'file', '/a.txt', 1, 1, '{\"path\":\"/a.txt\"}')",
        )
        .await
        .expect("pre-migration activity");
        crate::Migrator::up(&db, None).await.expect("apply");

        let row = db
            .query_one_raw(Statement::from_string(
                DatabaseBackend::Sqlite,
                "SELECT item_count, group_kind FROM activities WHERE commit_id = 'commit-1'",
            ))
            .await
            .expect("query")
            .expect("row");
        let count: i32 = row.try_get("", "item_count").expect("item_count");
        assert_eq!(count, 0, "legacy rows fall back to their detail JSON");
        let group_kind: String = row.try_get("", "group_kind").expect("group_kind");
        assert_eq!(group_kind, "session");

        let table = db
            .query_one_raw(Statement::from_string(
                DatabaseBackend::Sqlite,
                "SELECT name FROM sqlite_master \
                 WHERE type = 'table' AND name = 'activity_items'",
            ))
            .await
            .expect("query");
        assert!(table.is_some(), "activity_items exists");
        let _ = db.close().await;
    }
}
