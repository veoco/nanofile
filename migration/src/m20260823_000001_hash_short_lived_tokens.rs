use sea_orm::Statement;
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
        let backend = db.get_database_backend();
        db.execute(Statement::from_string(
            backend,
            "DELETE FROM sso_login_tokens".to_string(),
        ))
        .await?;
        db.execute(Statement::from_string(
            backend,
            "DELETE FROM client_login_tokens".to_string(),
        ))
        .await?;
        Ok(())
    }

    async fn down(&self, _manager: &SchemaManager) -> Result<(), DbErr> {
        // Data migration is not reversible (the plaintext tokens are gone).
        Ok(())
    }
}
