use async_trait::async_trait;
use sea_orm::{ActiveModelTrait, ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter, QueryOrder, Set};
use std::sync::Arc;

use crate::entity::api_token;
use crate::error::AppError;

/// Parameters for creating a session token.
pub struct CreateSessionTokenParams {
    pub user_id: i32,
    pub token: String,
    pub created_at: i64,
    pub expires_at: Option<i64>,
    pub device_id: Option<String>,
    pub platform: Option<String>,
    pub device_name: Option<String>,
    pub client_version: Option<String>,
}

#[async_trait]
pub trait ApiTokenRepository: Send + Sync {
    async fn find_by_token(&self, token: &str) -> Result<Option<api_token::Model>, AppError>;
    async fn find_by_user_id_with_platform(
        &self,
        user_id: i32,
    ) -> Result<Vec<api_token::Model>, AppError>;
    async fn delete_many_by_device(&self, device_id: &str) -> Result<(), AppError>;
    async fn delete_many_by_user_platform_device(
        &self,
        user_id: i32,
        platform: &str,
        device_id: &str,
    ) -> Result<u64, AppError>;
    async fn delete_by_token(&self, token: &str) -> Result<(), AppError>;
    async fn insert(&self, model: api_token::ActiveModel) -> Result<(), AppError>;
    async fn delete_many_by_user_id(&self, user_id: i32) -> Result<(), AppError>;

    // ── Methods for UI layer refactoring ───────────────────────────────
    /// Create a session token and return the model.
    async fn create_session_token(
        &self,
        params: CreateSessionTokenParams,
    ) -> Result<api_token::Model, AppError>;
}

pub struct DbApiTokenRepository {
    db: Arc<DatabaseConnection>,
}

impl DbApiTokenRepository {
    pub fn new(db: Arc<DatabaseConnection>) -> Self {
        Self { db }
    }
}

#[async_trait]
impl ApiTokenRepository for DbApiTokenRepository {
    async fn find_by_token(&self, token: &str) -> Result<Option<api_token::Model>, AppError> {
        Ok(api_token::Entity::find()
            .filter(api_token::Column::Token.eq(token))
            .one(self.db.as_ref())
            .await?)
    }

    async fn find_by_user_id_with_platform(
        &self,
        user_id: i32,
    ) -> Result<Vec<api_token::Model>, AppError> {
        Ok(api_token::Entity::find()
            .filter(api_token::Column::UserId.eq(user_id))
            .filter(api_token::Column::Platform.is_not_null())
            .order_by_desc(api_token::Column::CreatedAt)
            .all(self.db.as_ref())
            .await?)
    }

    async fn delete_many_by_device(&self, device_id: &str) -> Result<(), AppError> {
        api_token::Entity::delete_many()
            .filter(api_token::Column::DeviceId.eq(device_id))
            .exec(self.db.as_ref())
            .await?;
        Ok(())
    }

    async fn delete_by_token(&self, token: &str) -> Result<(), AppError> {
        api_token::Entity::delete_many()
            .filter(api_token::Column::Token.eq(token))
            .exec(self.db.as_ref())
            .await?;
        Ok(())
    }

    async fn delete_many_by_user_platform_device(
        &self,
        user_id: i32,
        platform: &str,
        device_id: &str,
    ) -> Result<u64, AppError> {
        let result = api_token::Entity::delete_many()
            .filter(api_token::Column::UserId.eq(user_id))
            .filter(api_token::Column::Platform.eq(platform))
            .filter(api_token::Column::DeviceId.eq(device_id))
            .exec(self.db.as_ref())
            .await?;
        Ok(result.rows_affected)
    }

    async fn insert(&self, model: api_token::ActiveModel) -> Result<(), AppError> {
        api_token::Entity::insert(model)
            .exec(self.db.as_ref())
            .await?;
        Ok(())
    }

    async fn delete_many_by_user_id(&self, user_id: i32) -> Result<(), AppError> {
        api_token::Entity::delete_many()
            .filter(api_token::Column::UserId.eq(user_id))
            .exec(self.db.as_ref())
            .await?;
        Ok(())
    }

    async fn create_session_token(
        &self,
        params: CreateSessionTokenParams,
    ) -> Result<api_token::Model, AppError> {
        let model = api_token::ActiveModel {
            id: sea_orm::NotSet,
            user_id: Set(params.user_id),
            token: Set(params.token),
            created_at: Set(params.created_at),
            expires_at: Set(params.expires_at),
            device_id: Set(params.device_id),
            platform: Set(params.platform),
            device_name: Set(params.device_name),
            client_version: Set(params.client_version),
        };
        Ok(model.insert(self.db.as_ref()).await?)
    }
}
