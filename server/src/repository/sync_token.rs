use async_trait::async_trait;
use sea_orm::{ActiveModelTrait, ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter, Set};
use std::sync::Arc;

use base::error::AppError;
use infra::crypto::token_encryption::TokenCipher;
use infra::entity::sync_token;

#[async_trait]
pub trait SyncTokenRepository: Send + Sync {
    async fn find_by_repo_and_user(
        &self,
        repo_id: &str,
        user_id: i32,
    ) -> Result<Option<sync_token::Model>, AppError>;
    /// Fetch tokens for many (repo, user) pairs in one query (chunked to stay
    /// under the SQLite variable limit). Used to batch `accessible_repos`.
    async fn find_by_repos_and_user(
        &self,
        repo_ids: &[String],
        user_id: i32,
    ) -> Result<Vec<sync_token::Model>, AppError>;
    /// Look up a token row by its **raw** (client-presented) value.
    async fn find_by_token(&self, token: &str) -> Result<Option<sync_token::Model>, AppError>;
    async fn find_by_token_and_repo(
        &self,
        token: &str,
        repo_id: &str,
    ) -> Result<Option<sync_token::Model>, AppError>;
    async fn create(
        &self,
        repo_id: &str,
        user_id: i32,
        token: String,
        client_peername: Option<String>,
        now: i64,
        expires_at: Option<i64>,
    ) -> Result<(), AppError>;
    async fn delete_by_repo(&self, repo_id: &str) -> Result<(), AppError>;
    /// Delete by the **raw** token value.
    async fn delete_by_token(&self, token: &str) -> Result<(), AppError>;
    async fn delete_by_id(&self, id: i32) -> Result<(), AppError>;
    async fn delete_by_user(&self, user_id: i32) -> Result<u64, AppError>;
    async fn delete_by_user_and_peer(&self, user_id: i32, peer_id: &str) -> Result<u64, AppError>;
    async fn update_peer_info(
        &self,
        model: sync_token::Model,
        peer_id: Option<String>,
        peer_name: Option<String>,
        peer_ip: Option<String>,
        client_version: Option<String>,
        last_sync_time: Option<i64>,
    ) -> Result<(), AppError>;
    /// Recover the raw token from a stored row. `None` when the ciphertext
    /// cannot be decrypted (e.g. the server secret changed) — callers must
    /// treat that as an invalid credential.
    fn reveal_token(&self, model: &sync_token::Model) -> Option<String>;
}

pub struct DbSyncTokenRepository {
    db: Arc<DatabaseConnection>,
    cipher: Arc<TokenCipher>,
}

impl DbSyncTokenRepository {
    pub fn new(db: Arc<DatabaseConnection>, cipher: Arc<TokenCipher>) -> Self {
        Self { db, cipher }
    }
}

#[async_trait]
impl SyncTokenRepository for DbSyncTokenRepository {
    async fn find_by_repo_and_user(
        &self,
        repo_id: &str,
        user_id: i32,
    ) -> Result<Option<sync_token::Model>, AppError> {
        Ok(sync_token::Entity::find()
            .filter(sync_token::Column::RepoId.eq(repo_id))
            .filter(sync_token::Column::UserId.eq(user_id))
            .one(self.db.as_ref())
            .await?)
    }

    async fn find_by_repos_and_user(
        &self,
        repo_ids: &[String],
        user_id: i32,
    ) -> Result<Vec<sync_token::Model>, AppError> {
        if repo_ids.is_empty() {
            return Ok(Vec::new());
        }
        // SQLite has a ~999 bound on bound parameters; chunk the IN list.
        const IN_BATCH: usize = 500;
        let mut out = Vec::new();
        for chunk in repo_ids.chunks(IN_BATCH) {
            let rows = sync_token::Entity::find()
                .filter(sync_token::Column::RepoId.is_in(chunk))
                .filter(sync_token::Column::UserId.eq(user_id))
                .all(self.db.as_ref())
                .await?;
            out.extend(rows);
        }
        Ok(out)
    }

    async fn find_by_token(&self, token: &str) -> Result<Option<sync_token::Model>, AppError> {
        // Deterministic AEAD: the stored value doubles as the lookup key, so a
        // client-presented raw token is encrypted before the equality query.
        let stored = self.cipher.encrypt(token);
        Ok(sync_token::Entity::find()
            .filter(sync_token::Column::Token.eq(stored))
            .one(self.db.as_ref())
            .await?)
    }

    async fn find_by_token_and_repo(
        &self,
        token: &str,
        repo_id: &str,
    ) -> Result<Option<sync_token::Model>, AppError> {
        let stored = self.cipher.encrypt(token);
        Ok(sync_token::Entity::find()
            .filter(sync_token::Column::Token.eq(stored))
            .filter(sync_token::Column::RepoId.eq(repo_id))
            .one(self.db.as_ref())
            .await?)
    }

    async fn create(
        &self,
        repo_id: &str,
        user_id: i32,
        token: String,
        peer_name: Option<String>,
        now: i64,
        expires_at: Option<i64>,
    ) -> Result<(), AppError> {
        sync_token::ActiveModel {
            id: sea_orm::NotSet,
            repo_id: Set(repo_id.to_string()),
            user_id: Set(user_id),
            token: Set(self.cipher.encrypt(&token)),
            peer_name: Set(peer_name),
            created_at: Set(now),
            expires_at: Set(expires_at),
            peer_id: Set(None),
            peer_ip: Set(None),
            client_version: Set(None),
            last_sync_time: Set(None),
        }
        .insert(self.db.as_ref())
        .await?;
        Ok(())
    }

    async fn delete_by_repo(&self, repo_id: &str) -> Result<(), AppError> {
        sync_token::Entity::delete_many()
            .filter(sync_token::Column::RepoId.eq(repo_id))
            .exec(self.db.as_ref())
            .await?;
        Ok(())
    }

    async fn delete_by_token(&self, token: &str) -> Result<(), AppError> {
        let stored = self.cipher.encrypt(token);
        sync_token::Entity::delete_many()
            .filter(sync_token::Column::Token.eq(stored))
            .exec(self.db.as_ref())
            .await?;
        Ok(())
    }

    async fn delete_by_id(&self, id: i32) -> Result<(), AppError> {
        sync_token::Entity::delete_many()
            .filter(sync_token::Column::Id.eq(id))
            .exec(self.db.as_ref())
            .await?;
        Ok(())
    }

    async fn delete_by_user(&self, user_id: i32) -> Result<u64, AppError> {
        let result = sync_token::Entity::delete_many()
            .filter(sync_token::Column::UserId.eq(user_id))
            .exec(self.db.as_ref())
            .await?;
        Ok(result.rows_affected)
    }

    async fn delete_by_user_and_peer(&self, user_id: i32, peer_id: &str) -> Result<u64, AppError> {
        let result = sync_token::Entity::delete_many()
            .filter(sync_token::Column::UserId.eq(user_id))
            .filter(sync_token::Column::PeerId.eq(peer_id))
            .exec(self.db.as_ref())
            .await?;
        Ok(result.rows_affected)
    }

    async fn update_peer_info(
        &self,
        model: sync_token::Model,
        peer_id: Option<String>,
        peer_name: Option<String>,
        peer_ip: Option<String>,
        client_version: Option<String>,
        last_sync_time: Option<i64>,
    ) -> Result<(), AppError> {
        sync_token::Entity::update_many()
            .filter(sync_token::Column::Id.eq(model.id))
            .set(sync_token::ActiveModel {
                peer_id: Set(peer_id),
                peer_name: Set(peer_name),
                peer_ip: Set(peer_ip),
                client_version: Set(client_version),
                last_sync_time: Set(last_sync_time),
                ..Default::default()
            })
            .exec(self.db.as_ref())
            .await?;
        Ok(())
    }

    fn reveal_token(&self, model: &sync_token::Model) -> Option<String> {
        self.cipher.decrypt(&model.token)
    }
}

/// One-time migration: encrypt every legacy plaintext sync-token row in place.
///
/// Clients keep presenting the same raw token, so this is non-disruptive.
/// Rows that are already encrypted (prefix [`TOKEN_CIPHER_PREFIX`]) are skipped.
/// Returns the number of rows rewritten.
pub async fn encrypt_legacy_sync_tokens(
    db: &DatabaseConnection,
    cipher: &TokenCipher,
) -> Result<u64, AppError> {
    use infra::crypto::token_encryption::TOKEN_CIPHER_PREFIX;

    let legacy = sync_token::Entity::find()
        .filter(sync_token::Column::Token.not_like(&format!("{TOKEN_CIPHER_PREFIX}%")))
        .all(db)
        .await?;

    let mut migrated = 0u64;
    for row in legacy {
        let stored = cipher.encrypt(&row.token);
        sync_token::Entity::update_many()
            .filter(sync_token::Column::Id.eq(row.id))
            .set(sync_token::ActiveModel {
                token: Set(stored),
                ..Default::default()
            })
            .exec(db)
            .await?;
        migrated += 1;
    }

    if migrated > 0 {
        tracing::info!(migrated, "encrypted legacy plaintext sync tokens at rest");
    }
    Ok(migrated)
}
