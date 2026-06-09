use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(InvitationCodes::Table)
                    .add_column(ColumnDef::new(InvitationCodes::Email).string().null())
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(InvitationCodes::Table)
                    .drop_column(InvitationCodes::Email)
                    .to_owned(),
            )
            .await
    }
}

#[derive(Iden)]
enum InvitationCodes {
    Table,
    Email,
}
