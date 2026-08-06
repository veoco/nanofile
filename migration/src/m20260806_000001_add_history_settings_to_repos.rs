use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        // SQLite doesn't support adding multiple columns in one ALTER TABLE.
        manager
            .alter_table(
                Table::alter()
                    .table(Repos::Table)
                    .add_column(
                        ColumnDef::new(Repos::HistoryLimit)
                            .integer()
                            .not_null()
                            .default(0),
                    )
                    .to_owned(),
            )
            .await?;
        manager
            .alter_table(
                Table::alter()
                    .table(Repos::Table)
                    .add_column(
                        ColumnDef::new(Repos::HistoryTtlDays)
                            .integer()
                            .not_null()
                            .default(0),
                    )
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(Repos::Table)
                    .drop_column(Repos::HistoryLimit)
                    .to_owned(),
            )
            .await?;
        manager
            .alter_table(
                Table::alter()
                    .table(Repos::Table)
                    .drop_column(Repos::HistoryTtlDays)
                    .to_owned(),
            )
            .await
    }
}

#[derive(Iden)]
enum Repos {
    Table,
    HistoryLimit,
    HistoryTtlDays,
}
