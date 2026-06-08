use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(ClientLoginTokens::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(ClientLoginTokens::Token)
                            .char_len(32)
                            .not_null()
                            .primary_key(),
                    )
                    .col(
                        ColumnDef::new(ClientLoginTokens::Username)
                            .string_len(255)
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(ClientLoginTokens::CreatedAt)
                            .big_integer()
                            .not_null(),
                    )
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(ClientLoginTokens::Table).to_owned())
            .await
    }
}

#[derive(Iden)]
enum ClientLoginTokens {
    Table,
    Token,
    Username,
    CreatedAt,
}
