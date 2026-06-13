use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(StarredFiles::Table)
                    .add_column(
                        ColumnDef::new(StarredFiles::IsDir)
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
                    .table(StarredFiles::Table)
                    .drop_column(StarredFiles::IsDir)
                    .to_owned(),
            )
            .await
    }
}

#[derive(Iden)]
enum StarredFiles {
    Table,
    IsDir,
}
