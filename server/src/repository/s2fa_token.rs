use async_trait::async_trait;
use sea_orm::{ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter};
use std::sync::Arc;

use base::error::AppError;
use infra::entity::s2fa_token;

#[async_trait]
pub trait S2faTokenRepository: Send + Sync {
    async fn find_by_token_and_user(
        &self,
        token: &str,
        user_id: i32,
    ) -> Result<Option<s2fa_token::Model>, AppError>;
    async fn delete_expired(&self, user_id: i32, now: i64) -> Result<(), AppError>;
    async fn delete_by_user_and_device(
        &self,
        user_id: i32,
        device_id: &str,
    ) -> Result<u64, AppError>;
    async fn insert(&self, model: s2fa_token::ActiveModel) -> Result<(), AppError>;
}

pub struct DbS2faTokenRepository {
    db: Arc<DatabaseConnection>,
}

impl DbS2faTokenRepository {
    pub fn new(db: Arc<DatabaseConnection>) -> Self {
        Self { db }
    }
}

#[async_trait]
impl S2faTokenRepository for DbS2faTokenRepository {
    async fn find_by_token_and_user(
        &self,
        token: &str,
        user_id: i32,
    ) -> Result<Option<s2fa_token::Model>, AppError> {
        Ok(s2fa_token::Entity::find()
            .filter(s2fa_token::Column::Token.eq(token))
            .filter(s2fa_token::Column::UserId.eq(user_id))
            .one(self.db.as_ref())
            .await?)
    }

    async fn delete_expired(&self, user_id: i32, now: i64) -> Result<(), AppError> {
        s2fa_token::Entity::delete_many()
            .filter(s2fa_token::Column::UserId.eq(user_id))
            .filter(s2fa_token::Column::ExpiresAt.lt(now))
            .exec(self.db.as_ref())
            .await?;
        Ok(())
    }

    async fn delete_by_user_and_device(
        &self,
        user_id: i32,
        device_id: &str,
    ) -> Result<u64, AppError> {
        let result = s2fa_token::Entity::delete_many()
            .filter(s2fa_token::Column::UserId.eq(user_id))
            .filter(s2fa_token::Column::DeviceId.eq(device_id))
            .exec(self.db.as_ref())
            .await?;
        Ok(result.rows_affected)
    }

    async fn insert(&self, model: s2fa_token::ActiveModel) -> Result<(), AppError> {
        s2fa_token::Entity::insert(model)
            .exec(self.db.as_ref())
            .await?;
        Ok(())
    }
}
