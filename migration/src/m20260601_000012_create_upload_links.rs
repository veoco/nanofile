use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(UploadLinks::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(UploadLinks::Id)
                            .integer()
                            .not_null()
                            .auto_increment()
                            .primary_key(),
                    )
                    .col(
                        ColumnDef::new(UploadLinks::RepoId)
                            .char_len(36)
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(UploadLinks::CreatorId)
                            .integer()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(UploadLinks::Path)
                            .string_len(4096)
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(UploadLinks::Token)
                            .char_len(24)
                            .not_null()
                            .unique_key(),
                    )
                    .col(ColumnDef::new(UploadLinks::Password).string_len(512))
                    .col(ColumnDef::new(UploadLinks::ExpiresAt).big_integer())
                    .col(
                        ColumnDef::new(UploadLinks::CreatedAt)
                            .big_integer()
                            .not_null(),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_upload_links_repo_id")
                            .from(UploadLinks::Table, UploadLinks::RepoId)
                            .to(Repos::Table, Repos::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_upload_links_creator_id")
                            .from(UploadLinks::Table, UploadLinks::CreatorId)
                            .to(Users::Table, Users::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(UploadLinks::Table).to_owned())
            .await
    }
}

#[derive(Iden)]
enum UploadLinks {
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
