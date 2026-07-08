use async_trait::async_trait;
use sea_orm::{ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter, QueryOrder};
use std::sync::Arc;

use crate::entity::api_token;
use crate::error::AppError;

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
}
