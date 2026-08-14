use sea_orm_migration::prelude::*;

/// No-op placeholder for the removed seadoc comment table.
///
/// The original migration created `sdoc_comments`, but seadoc support was
/// removed afterwards (entity, repository, service and this migration were
/// deleted together). Deleting an *already-applied* migration breaks the
/// migrator: deployed databases still record this version in `seaql_migrations`
/// and then report the file as missing, blocking every later migration. This
/// no-op keeps the migration chain intact for both fresh and already-deployed
/// databases. The leftover `sdoc_comments` table is dropped by
/// m20260814_000006_drop_sdoc_comments.
#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, _manager: &SchemaManager) -> Result<(), DbErr> {
        Ok(())
    }

    async fn down(&self, _manager: &SchemaManager) -> Result<(), DbErr> {
        Ok(())
    }
}
