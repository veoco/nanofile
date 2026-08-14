use sea_orm_migration::prelude::*;

/// Seafile wiki2 public-publish configuration: one row per published wiki.
/// Mirrors seahub's `wiki2_publish` table.
#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(Wiki2Publish::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(Wiki2Publish::RepoId)
                            .char_len(36)
                            .not_null()
                            .primary_key(),
                    )
                    .col(
                        ColumnDef::new(Wiki2Publish::PublishUrl)
                            .string_len(40)
                            .not_null()
                            .unique_key(),
                    )
                    .col(ColumnDef::new(Wiki2Publish::Username).string_len(255).not_null())
                    .col(
                        ColumnDef::new(Wiki2Publish::CreatedAt)
                            .big_integer()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(Wiki2Publish::VisitCount)
                            .integer()
                            .not_null()
                            .default(0),
                    )
                    .col(
                        ColumnDef::new(Wiki2Publish::EnableServerRender)
                            .boolean()
                            .not_null()
                            .default(false),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_wiki2_publish_repo_id")
                            .from(Wiki2Publish::Table, Wiki2Publish::RepoId)
                            .to(Repos::Table, Repos::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(Wiki2Publish::Table).to_owned())
            .await
    }
}

#[derive(Iden)]
enum Wiki2Publish {
    Table,
    RepoId,
    PublishUrl,
    Username,
    CreatedAt,
    VisitCount,
    EnableServerRender,
}

#[derive(Iden)]
enum Repos {
    Table,
    Id,
}
