use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(ShareLinks::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(ShareLinks::Id)
                            .integer()
                            .not_null()
                            .auto_increment()
                            .primary_key(),
                    )
                    .col(ColumnDef::new(ShareLinks::RepoId).char_len(36).not_null())
                    .col(ColumnDef::new(ShareLinks::CreatorId).integer().not_null())
                    .col(ColumnDef::new(ShareLinks::Path).string_len(4096).not_null())
                    .col(
                        ColumnDef::new(ShareLinks::Token)
                            .char_len(16)
                            .not_null()
                            .unique_key(),
                    )
                    .col(ColumnDef::new(ShareLinks::Password).string_len(512))
                    .col(ColumnDef::new(ShareLinks::ExpiresAt).big_integer())
                    .col(
                        ColumnDef::new(ShareLinks::CreatedAt)
                            .big_integer()
                            .not_null(),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_share_links_repo_id")
                            .from(ShareLinks::Table, ShareLinks::RepoId)
                            .to(Repos::Table, Repos::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_share_links_creator_id")
                            .from(ShareLinks::Table, ShareLinks::CreatorId)
                            .to(Users::Table, Users::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(ShareLinks::Table).to_owned())
            .await
    }
}

#[derive(Iden)]
enum ShareLinks {
    Table,
    Id,
    RepoId,
    CreatorId,
    Path,
    Token,
    Password,
    ExpiresAt,
    CreatedAt,
}

#[derive(Iden)]
enum Repos {
    Table,
    Id,
}

#[derive(Iden)]
enum Users {
    Table,
    Id,
}
