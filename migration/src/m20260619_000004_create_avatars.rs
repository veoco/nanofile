use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(Avatars::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(Avatars::Id)
                            .integer()
                            .not_null()
                            .auto_increment()
                            .primary_key(),
                    )
                    .col(
                        ColumnDef::new(Avatars::Email)
                            .string_len(255)
                            .not_null()
                            .unique_key(),
                    )
                    .col(
                        ColumnDef::new(Avatars::AvatarFileName)
                            .string_len(512)
                            .not_null(),
                    )
                    .col(ColumnDef::new(Avatars::MimeType).string_len(64).not_null())
                    .col(ColumnDef::new(Avatars::FileSize).integer().not_null())
                    .col(
                        ColumnDef::new(Avatars::DateUploaded)
                            .big_integer()
                            .not_null(),
                    )
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(Avatars::Table).to_owned())
            .await
    }
}

#[derive(Iden)]
enum Avatars {
    Table,
    Id,
    Email,
    AvatarFileName,
    MimeType,
    FileSize,
    DateUploaded,
}
