use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(WebdavKeys::Table)
                    .add_column(
                        ColumnDef::new(WebdavKeys::Permission)
                            .string_len(2)
                            .not_null()
                            .default("rw"),
                    )
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(WebdavKeys::Table)
                    .drop_column(WebdavKeys::Permission)
                    .to_owned(),
            )
            .await
    }
}

#[derive(Iden)]
enum WebdavKeys {
    Table,
    Permission,
}
