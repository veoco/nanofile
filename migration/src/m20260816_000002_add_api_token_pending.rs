use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(ApiTokens::Table)
                    .add_column(
                        ColumnDef::new(ApiTokens::IsPending)
                            .boolean()
                            .not_null()
                            .default(false),
                    )
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(ApiTokens::Table)
                    .drop_column(ApiTokens::IsPending)
                    .to_owned(),
            )
            .await
    }
}

#[derive(Iden)]
enum ApiTokens {
    Table,
    IsPending,
}
