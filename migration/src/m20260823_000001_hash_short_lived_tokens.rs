use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        // SSO and client-login tokens are now stored as SHA-256 hashes instead
        // of plaintext. Purge the existing plaintext rows so no raw bearer
        // credential survives in the database; both are one-shot, short-lived
        // tokens, so in-flight flows simply restart.
        let db = manager.get_connection();
        db.execute_unprepared("DELETE FROM sso_login_tokens").await?;
        db.execute_unprepared("DELETE FROM client_login_tokens").await?;
        Ok(())
    }

    async fn down(&self, _manager: &SchemaManager) -> Result<(), DbErr> {
        // Data migration is not reversible (the plaintext tokens are gone).
        Ok(())
    }
}
