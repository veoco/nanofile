use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(ApiTokens::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(ApiTokens::Id)
                            .integer()
                            .not_null()
                            .auto_increment()
                            .primary_key(),
                    )
                    .col(ColumnDef::new(ApiTokens::UserId).integer().not_null())
                    .col(
                        ColumnDef::new(ApiTokens::Token)
                            .char_len(40)
                            .not_null()
                            .unique_key(),
                    )
                    .col(
                        ColumnDef::new(ApiTokens::CreatedAt)
                            .big_integer()
                            .not_null(),
                    )
                    .col(ColumnDef::new(ApiTokens::ExpiresAt).big_integer())
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_api_tokens_user_id")
                            .from(ApiTokens::Table, ApiTokens::UserId)
                            .to(Users::Table, Users::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(ApiTokens::Table).to_owned())
            .await
    }
}

#[derive(Iden)]
enum ApiTokens {
    Table,
    Id,
    UserId,
    Token,
    CreatedAt,
    ExpiresAt,
}

#[derive(Iden)]
enum Users {
    Table,
    Id,
}
