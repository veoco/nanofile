use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(S2faTokens::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(S2faTokens::Id)
                            .integer()
                            .not_null()
                            .auto_increment()
                            .primary_key(),
                    )
                    .col(
                        ColumnDef::new(S2faTokens::UserId)
                            .integer()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(S2faTokens::Token)
                            .char_len(40)
                            .not_null()
                            .unique_key(),
                    )
                    .col(ColumnDef::new(S2faTokens::DeviceId).string_len(255))
                    .col(ColumnDef::new(S2faTokens::DeviceName).string_len(255))
                    .col(
                        ColumnDef::new(S2faTokens::CreatedAt)
                            .big_integer()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(S2faTokens::ExpiresAt)
                            .big_integer()
                            .not_null(),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_s2fa_tokens_user_id")
                            .from(S2faTokens::Table, S2faTokens::UserId)
                            .to(Users::Table, Users::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(S2faTokens::Table).to_owned())
            .await
    }
}

#[derive(Iden)]
enum S2faTokens {
    Table,
    Id,
    UserId,
    Token,
    DeviceId,
    DeviceName,
    CreatedAt,
    ExpiresAt,
}

#[derive(Iden)]
enum Users {
    Table,
    Id,
}
