use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(Commits::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(Commits::Id)
                            .big_integer()
                            .not_null()
                            .auto_increment()
                            .primary_key(),
                    )
                    .col(ColumnDef::new(Commits::RepoId).char_len(36).not_null())
                    .col(ColumnDef::new(Commits::CommitId).char_len(40).not_null())
                    .col(ColumnDef::new(Commits::RootId).char_len(40).not_null())
                    .col(ColumnDef::new(Commits::ParentId).char_len(40))
                    .col(ColumnDef::new(Commits::SecondParentId).char_len(40))
                    .col(
                        ColumnDef::new(Commits::CreatorName)
                            .string_len(255)
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(Commits::Description)
                            .string_len(4096)
                            .not_null()
                            .default(""),
                    )
                    .col(ColumnDef::new(Commits::Ctime).big_integer().not_null())
                    .col(
                        ColumnDef::new(Commits::Version)
                            .tiny_integer()
                            .not_null()
                            .default(1),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_commits_repo_id")
                            .from(Commits::Table, Commits::RepoId)
                            .to(Repos::Table, Repos::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .name("idx_commits_repo_commit")
                    .table(Commits::Table)
                    .col(Commits::RepoId)
                    .col(Commits::CommitId)
                    .unique()
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(Commits::Table).to_owned())
            .await
    }
}

#[derive(Iden)]
enum Commits {
    Table,
    Id,
    RepoId,
    CommitId,
    RootId,
    ParentId,
    SecondParentId,
    CreatorName,
    Description,
    Ctime,
    Version,
}

#[derive(Iden)]
enum Repos {
    Table,
    Id,
}
