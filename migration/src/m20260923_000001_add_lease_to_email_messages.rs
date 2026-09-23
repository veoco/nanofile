use sea_orm_migration::prelude::*;

/// Give the outbox a claim lease.
///
/// Delivery is claimed with a compare-and-swap on `attempts`, which is not
/// exclusive on its own: a second drain that reads the row *after* the first
/// claim commits sees the incremented counter and its own swap succeeds, so
/// both send. A lease makes the claim exclusive, and an expired lease is what
/// lets a row whose holder died become claimable again.
#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(EmailMessages::Table)
                    .add_column(ColumnDef::new(EmailMessages::LeaseUntil).big_integer().null())
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(EmailMessages::Table)
                    .drop_column(EmailMessages::LeaseUntil)
                    .to_owned(),
            )
            .await
    }
}

#[derive(Iden)]
enum EmailMessages {
    Table,
    LeaseUntil,
}
