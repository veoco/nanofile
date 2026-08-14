use sea_orm_migration::prelude::*;

/// Drop the dead `dir_entries` table.
///
/// Created by m20260601_000008, the table has no corresponding entity and is
/// never queried by the server (directory entries live inside `fs_objects`
/// JSON blobs). Its three indexes and foreign key are dropped along with it.
/// `IF EXISTS` keeps this safe for databases that predate the feature or have
/// already dropped the table manually.
#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(DirEntries::Table).if_exists().to_owned())
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        // Re-create the table structure for reversibility (informational only).
        manager
            .create_table(
                Table::create()
                    .table(DirEntries::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(DirEntries::Id)
                            .big_integer()
                            .not_null()
                            .auto_increment()
                            .primary_key(),
                    )
                    .col(ColumnDef::new(DirEntries::RepoId).char_len(36).not_null())
                    .col(ColumnDef::new(DirEntries::ParentId).char_len(40).not_null())
                    .col(ColumnDef::new(DirEntries::ChildId).char_len(40).not_null())
                    .col(ColumnDef::new(DirEntries::Name).string_len(255).not_null())
                    .col(ColumnDef::new(DirEntries::EntryType).tiny_integer().not_null())
                    .col(ColumnDef::new(DirEntries::Mode).integer().not_null())
                    .col(
                        ColumnDef::new(DirEntries::Size)
                            .big_integer()
                            .not_null()
                            .default(0),
                    )
                    .col(ColumnDef::new(DirEntries::Mtime).big_integer().not_null())
                    .col(ColumnDef::new(DirEntries::Modifier).string_len(255).not_null())
                    .col(ColumnDef::new(DirEntries::Path).string_len(4096).not_null())
                    .to_owned(),
            )
            .await
    }
}

#[derive(Iden)]
enum DirEntries {
    Table,
    Id,
    RepoId,
    ParentId,
    ChildId,
    Name,
    EntryType,
    Mode,
    Size,
    Mtime,
    Modifier,
    Path,
}
