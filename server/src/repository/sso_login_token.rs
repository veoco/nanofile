use async_trait::async_trait;
use sea_orm::{ActiveModelTrait, ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter, Set};
use std::sync::Arc;

use base::error::AppError;
use infra::entity::sso_login_token;

pub struct CreateSsoLoginTokenParams {
    pub token: String,
    pub platform: Option<String>,
    pub device_id: Option<String>,
    pub device_name: Option<String>,
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
}

pub struct DbSsoLoginTokenRepository {
    db: Arc<DatabaseConnection>,
}

impl DbSsoLoginTokenRepository {
    pub fn new(db: Arc<DatabaseConnection>) -> Self {
        Self { db }
    }
}

#[async_trait]
impl SsoLoginTokenRepository for DbSsoLoginTokenRepository {
    async fn find_by_token(&self, token: &str) -> Result<Option<sso_login_token::Model>, AppError> {
        Ok(sso_login_token::Entity::find()
            .filter(sso_login_token::Column::Token.eq(token))
            .one(self.db.as_ref())
            .await?)
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
            token: Set(params.token),
            platform: Set(params.platform),
            device_id: Set(params.device_id),
            device_name: Set(params.device_name),
            status: Set(params.status),
            username: Set(params.username),
            api_token: Set(params.api_token),
            created_at: Set(params.created_at),
            expires_at: Set(params.expires_at),
        };
        Ok(model.insert(self.db.as_ref()).await?)
    }
}
