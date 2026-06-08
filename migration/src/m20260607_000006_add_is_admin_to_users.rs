use sea_orm::Statement;
use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(Users::Table)
                    .add_column(
                        ColumnDef::new(Users::IsAdmin)
                            .boolean()
                            .not_null()
                            .default(false),
                    )
                    .to_owned(),
            )
            .await?;

        // Mark existing root users (invited_by IS NULL) as admins.
        let db = manager.get_connection();
        let backend = db.get_database_backend();
        db.execute(Statement::from_string(
            backend,
            "UPDATE users SET is_admin = 1 WHERE invited_by IS NULL".to_string(),
        ))
        .await?;

        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(Users::Table)
                    .drop_column(Users::IsAdmin)
                    .to_owned(),
            )
            .await
    }
}

#[derive(Iden)]
enum Users {
    Table,
    IsAdmin,
}
