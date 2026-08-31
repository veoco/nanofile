use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        // API and S2FA tokens are now stored as SHA-256 hashes instead of
        // plaintext. Purge the existing plaintext rows so no plaintext bearer
        // credential survives in the database; affected users simply log in
        // again (and trusted devices re-run 2FA once).
        let db = manager.get_connection();
        db.execute_unprepared("DELETE FROM api_tokens").await?;
        db.execute_unprepared("DELETE FROM s2fa_tokens").await?;
        Ok(())
    }

    async fn down(&self, _manager: &SchemaManager) -> Result<(), DbErr> {
        // Data migration is not reversible (the plaintext tokens are gone).
        Ok(())
    }
}
