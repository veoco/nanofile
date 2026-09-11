use async_trait::async_trait;
use sea_orm::{ActiveModelTrait, ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter, Set};
use std::sync::Arc;

use crate::service::auth::token::hash_token;
use base::error::AppError;
use infra::crypto::token_encryption::TokenCipher;
use infra::entity::sso_login_token;

pub struct CreateSsoLoginTokenParams {
    pub token: String,
    pub platform: Option<String>,
    pub device_id: Option<String>,
    pub device_name: Option<String>,
    pub client_version: Option<String>,
    pub status: String,
    pub username: Option<String>,
    pub api_token: Option<String>,
    pub created_at: i64,
    pub expires_at: Option<i64>,
}

#[async_trait]
pub trait SsoLoginTokenRepository: Send + Sync {
    async fn find_by_token(&self, token: &str) -> Result<Option<sso_login_token::Model>, AppError>;
    async fn insert(&self, model: sso_login_token::ActiveModel) -> Result<(), AppError>;
    async fn create_sso_token(
        &self,
        params: CreateSsoLoginTokenParams,
    ) -> Result<sso_login_token::Model, AppError>;
    /// Record that the browser opened `/client-sso/{token}/` (start of the
    /// 300s completion window).
    async fn mark_accessed(&self, token: &str, accessed_at: i64) -> Result<(), AppError>;
    /// Mark the SSO flow successful with the completed username and API token.
    async fn complete(&self, token: &str, username: &str, api_token: &str) -> Result<(), AppError>;
}

pub struct DbSsoLoginTokenRepository {
    db: Arc<DatabaseConnection>,
    /// The API token minted by [`complete`](Self::complete) is a live bearer
    /// credential, and unlike the token in `api_tokens` it cannot be hashed:
    /// the polling client must receive the raw value. It is therefore encrypted
    /// at rest so a database copy (backup, disk image) does not hand out
    /// long-lived tokens.
    token_cipher: Arc<TokenCipher>,
}

impl DbSsoLoginTokenRepository {
    pub fn new(db: Arc<DatabaseConnection>, token_cipher: Arc<TokenCipher>) -> Self {
        Self { db, token_cipher }
    }
}

#[async_trait]
impl SsoLoginTokenRepository for DbSsoLoginTokenRepository {
    async fn find_by_token(&self, token: &str) -> Result<Option<sso_login_token::Model>, AppError> {
        let record = sso_login_token::Entity::find()
            .filter(sso_login_token::Column::Token.eq(hash_token(token)))
            .one(self.db.as_ref())
            .await?;

        // Hand callers the plaintext token. A value that cannot be decrypted
        // (rotated server secret, tampered row) is treated as "no token yet"
        // rather than an internal error, so the flow reports `error` to the
        // client instead of a 500.
        Ok(record.map(|mut record| {
            if let Some(stored) = record.api_token.take() {
                match self.token_cipher.decrypt(&stored) {
                    Some(plaintext) => record.api_token = Some(plaintext),
                    None => {
                        tracing::warn!(
                            "sso_login_token {}: stored api_token could not be decrypted",
                            record.id
                        );
                    }
                }
            }
            record
        }))
    }

    async fn insert(&self, model: sso_login_token::ActiveModel) -> Result<(), AppError> {
        sso_login_token::Entity::insert(model)
            .exec(self.db.as_ref())
            .await?;
        Ok(())
    }

    async fn create_sso_token(
        &self,
        params: CreateSsoLoginTokenParams,
    ) -> Result<sso_login_token::Model, AppError> {
        let model = sso_login_token::ActiveModel {
            id: sea_orm::NotSet,
            token: Set(hash_token(&params.token)),
            platform: Set(params.platform),
            device_id: Set(params.device_id),
            device_name: Set(params.device_name),
            client_version: Set(params.client_version),
            status: Set(params.status),
            username: Set(params.username),
            api_token: Set(params
                .api_token
                .as_deref()
                .map(|t| self.token_cipher.encrypt(t))),
            created_at: Set(params.created_at),
            expires_at: Set(params.expires_at),
            accessed_at: Set(None),
        };
        Ok(model.insert(self.db.as_ref()).await?)
    }

    async fn mark_accessed(&self, token: &str, accessed_at: i64) -> Result<(), AppError> {
        sso_login_token::Entity::update_many()
            .set(sso_login_token::ActiveModel {
                accessed_at: Set(Some(accessed_at)),
                ..Default::default()
            })
            .filter(sso_login_token::Column::Token.eq(hash_token(token)))
            .exec(self.db.as_ref())
            .await?;
        Ok(())
    }

    async fn complete(&self, token: &str, username: &str, api_token: &str) -> Result<(), AppError> {
        sso_login_token::Entity::update_many()
            .set(sso_login_token::ActiveModel {
                status: Set("success".to_string()),
                username: Set(Some(username.to_string())),
                api_token: Set(Some(self.token_cipher.encrypt(api_token))),
                ..Default::default()
            })
            .filter(sso_login_token::Column::Token.eq(hash_token(token)))
            .exec(self.db.as_ref())
            .await?;
        Ok(())
    }
}
