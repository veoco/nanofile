use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(DeletedRepos::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(DeletedRepos::RepoId)
                            .char_len(36)
                            .not_null()
                            .primary_key(),
                    )
                    .col(ColumnDef::new(DeletedRepos::RepoName).string_len(255).not_null())
                    .col(ColumnDef::new(DeletedRepos::HeadId).char_len(40).null())
                    .col(ColumnDef::new(DeletedRepos::OwnerId).integer().not_null())
                    .col(ColumnDef::new(DeletedRepos::Size).big_integer().not_null().default(0))
                    .col(ColumnDef::new(DeletedRepos::DelTime).big_integer().not_null())
                    .to_owned(),
            )
            .await?;

        // Index for looking up trashed repos by owner
        manager
            .create_index(
                Index::create()
                    .name("idx_deleted_repos_owner_time")
                    .table(DeletedRepos::Table)
                    .col(DeletedRepos::OwnerId)
                    .col(DeletedRepos::DelTime)
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(DeletedRepos::Table).to_owned())
            .await
    }
}

#[derive(Iden)]
enum DeletedRepos {
    Table,
    RepoId,
    RepoName,
    HeadId,
    OwnerId,
    Size,
    DelTime,
}
