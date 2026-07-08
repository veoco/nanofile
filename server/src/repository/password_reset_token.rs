use async_trait::async_trait;
use sea_orm::{ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter};
use std::sync::Arc;

use crate::entity::password_reset_token;
use crate::error::AppError;

#[async_trait]
pub trait PasswordResetTokenRepository: Send + Sync {
    async fn find_by_token_hash(
        &self,
        token_hash: &str,
    ) -> Result<Option<password_reset_token::Model>, AppError>;
    async fn find_by_user(
        &self,
        user_id: i32,
    ) -> Result<Vec<password_reset_token::Model>, AppError>;
    async fn delete_by_id(&self, id: i32) -> Result<(), AppError>;
}

pub struct DbPasswordResetTokenRepository {
    db: Arc<DatabaseConnection>,
}

impl DbPasswordResetTokenRepository {
    pub fn new(db: Arc<DatabaseConnection>) -> Self {
        Self { db }
    }
}

#[async_trait]
impl PasswordResetTokenRepository for DbPasswordResetTokenRepository {
    async fn find_by_token_hash(
        &self,
        token_hash: &str,
    ) -> Result<Option<password_reset_token::Model>, AppError> {
        Ok(password_reset_token::Entity::find()
            .filter(password_reset_token::Column::TokenHash.eq(token_hash))
            .one(self.db.as_ref())
            .await?)
    }

    async fn find_by_user(
        &self,
        user_id: i32,
    ) -> Result<Vec<password_reset_token::Model>, AppError> {
        Ok(password_reset_token::Entity::find()
            .filter(password_reset_token::Column::UserId.eq(user_id))
            .all(self.db.as_ref())
            .await?)
    }

    async fn delete_by_id(&self, id: i32) -> Result<(), AppError> {
        password_reset_token::Entity::delete_by_id(id)
            .exec(self.db.as_ref())
            .await?;
        Ok(())
    }
}
