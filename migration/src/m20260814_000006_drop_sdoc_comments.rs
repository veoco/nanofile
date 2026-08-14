use sea_orm_migration::prelude::*;

/// Drop the leftover `sdoc_comments` table from the removed seadoc feature.
///
/// The table was created by m20260604_000003 (now a no-op placeholder) before
/// seadoc support was removed. Already-deployed databases still carry the empty
/// table; fresh databases never create it. `IF EXISTS` makes this safe for both.
#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table("sdoc_comments").if_exists().to_owned())
            .await
    }

    async fn down(&self, _manager: &SchemaManager) -> Result<(), DbErr> {
        // seadoc is removed entirely; there is nothing meaningful to restore.
        Ok(())
    }
}
