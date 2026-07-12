use async_trait::async_trait;
use sea_orm::{ActiveModelTrait, ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter, Set};
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

    // ── Methods for UI layer refactoring ───────────────────────────────
    /// Create a new password reset token.
    async fn create(
        &self,
        user_id: i32,
        token_hash: String,
        created_at: i64,
        expires_at: i64,
    ) -> Result<password_reset_token::Model, AppError>;
    /// Mark a token as used.
    async fn mark_as_used(&self, id: i32) -> Result<(), AppError>;
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

    async fn create(
        &self,
        user_id: i32,
        token_hash: String,
        created_at: i64,
        expires_at: i64,
    ) -> Result<password_reset_token::Model, AppError> {
        let model = password_reset_token::ActiveModel {
            id: sea_orm::NotSet,
            user_id: Set(user_id),
            token_hash: Set(token_hash),
            created_at: Set(created_at),
            expires_at: Set(expires_at),
            used: Set(false),
        };
        Ok(model.insert(self.db.as_ref()).await?)
    }

    async fn mark_as_used(&self, id: i32) -> Result<(), AppError> {
        let result = password_reset_token::Entity::update_many()
            .filter(password_reset_token::Column::Id.eq(id))
            .set(password_reset_token::ActiveModel {
                used: Set(true),
                ..Default::default()
            })
            .exec(self.db.as_ref())
            .await?;
        if result.rows_affected == 0 {
            return Err(AppError::NotFound("Token not found.".to_string()));
        }
        Ok(())
    }
}
