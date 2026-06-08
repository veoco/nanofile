use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
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
                    .col(
                        ColumnDef::new(DirEntries::RepoId)
                            .char_len(36)
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(DirEntries::ParentId)
                            .char_len(40)
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(DirEntries::ChildId)
                            .char_len(40)
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(DirEntries::Name)
                            .string_len(255)
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(DirEntries::EntryType)
                            .tiny_integer()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(DirEntries::Mode)
                            .integer()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(DirEntries::Size)
                            .big_integer()
                            .not_null()
                            .default(0),
                    )
                    .col(
                        ColumnDef::new(DirEntries::Mtime)
                            .big_integer()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(DirEntries::Modifier)
                            .string_len(255)
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(DirEntries::Path)
                            .string_len(4096)
                            .not_null(),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_dir_entries_repo_id")
                            .from(DirEntries::Table, DirEntries::RepoId)
                            .to(Repos::Table, Repos::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .name("idx_dir_entries_repo_path")
                    .table(DirEntries::Table)
                    .col(DirEntries::RepoId)
                    .col(DirEntries::Path)
                    .unique()
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .name("idx_dir_entries_repo_parent")
                    .table(DirEntries::Table)
                    .col(DirEntries::RepoId)
                    .col(DirEntries::ParentId)
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .name("idx_dir_entries_repo_name")
                    .table(DirEntries::Table)
                    .col(DirEntries::RepoId)
                    .col(DirEntries::Name)
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(DirEntries::Table).to_owned())
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

#[derive(Iden)]
enum Repos {
    Table,
    Id,
}
