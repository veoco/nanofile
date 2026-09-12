use sea_orm_migration::prelude::*;

/// Record the last TOTP time step accepted for a user, so the same (or an
/// earlier) code cannot be replayed within the ±1-step acceptance window.
#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(User2fa::Table)
                    .add_column(ColumnDef::new(User2fa::LastUsedStep).big_integer().null())
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(User2fa::Table)
                    .drop_column(User2fa::LastUsedStep)
                    .to_owned(),
            )
            .await
    }
}

#[derive(Iden)]
enum User2fa {
    #[iden = "user_2fa"]
    Table,
    LastUsedStep,
}
