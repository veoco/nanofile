use sea_orm_migration::prelude::*;

/// A single-row-per-key table for state the server keeps about itself rather than
/// about a user or a library. The first entry records which master secret the
/// stored sessions belong to, so a changed `server.secret_key` can revoke them at
/// start-up (see `service::auth::session_fingerprint`).
#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(InstanceMeta::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(InstanceMeta::Key)
                            .text()
                            .not_null()
                            .primary_key(),
                    )
                    .col(ColumnDef::new(InstanceMeta::Value).text().not_null())
                    .to_owned(),
            )
            .await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(InstanceMeta::Table).to_owned())
            .await?;
        Ok(())
    }
}

#[derive(DeriveIden)]
enum InstanceMeta {
    Table,
    Key,
    Value,
}
