use async_trait::async_trait;
use sea_orm::{
    ActiveModelTrait, ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter, QueryOrder, Set,
};
use std::sync::Arc;

use base::error::AppError;
use infra::entity::email_verification_token;

#[async_trait]
pub trait EmailVerificationTokenRepository: Send + Sync {
    async fn find_by_token_hash(
        &self,
        token_hash: &str,
    ) -> Result<Option<email_verification_token::Model>, AppError>;
    async fn find_by_user(
        &self,
        user_id: i32,
    ) -> Result<Vec<email_verification_token::Model>, AppError>;
    /// Creation time of the newest token for `user_id`, if one exists. The login
    /// path uses it to keep a just-sent link from being replaced immediately.
    async fn last_created_at(&self, user_id: i32) -> Result<Option<i64>, AppError>;
    async fn delete_by_user(&self, user_id: i32) -> Result<(), AppError>;

    async fn create(
        &self,
        user_id: i32,
        token_hash: String,
        created_at: i64,
        expires_at: i64,
    ) -> Result<email_verification_token::Model, AppError>;
    /// Atomically claim a token, returning `true` only for the caller that won.
    async fn mark_as_used(&self, id: i32) -> Result<bool, AppError>;
}

pub struct DbEmailVerificationTokenRepository {
    db: Arc<DatabaseConnection>,
}

impl DbEmailVerificationTokenRepository {
    pub fn new(db: Arc<DatabaseConnection>) -> Self {
        Self { db }
    }
}

#[async_trait]
impl EmailVerificationTokenRepository for DbEmailVerificationTokenRepository {
    async fn find_by_token_hash(
        &self,
        token_hash: &str,
    ) -> Result<Option<email_verification_token::Model>, AppError> {
        Ok(email_verification_token::Entity::find()
            .filter(email_verification_token::Column::TokenHash.eq(token_hash))
            .one(self.db.as_ref())
            .await?)
    }

    async fn find_by_user(
        &self,
        user_id: i32,
    ) -> Result<Vec<email_verification_token::Model>, AppError> {
        Ok(email_verification_token::Entity::find()
            .filter(email_verification_token::Column::UserId.eq(user_id))
            .order_by_desc(email_verification_token::Column::CreatedAt)
            .all(self.db.as_ref())
            .await?)
    }

    async fn last_created_at(&self, user_id: i32) -> Result<Option<i64>, AppError> {
        let row = email_verification_token::Entity::find()
            .filter(email_verification_token::Column::UserId.eq(user_id))
            .order_by_desc(email_verification_token::Column::CreatedAt)
            .one(self.db.as_ref())
            .await?;
        Ok(row.map(|row| row.created_at))
    }

    async fn delete_by_user(&self, user_id: i32) -> Result<(), AppError> {
        email_verification_token::Entity::delete_many()
            .filter(email_verification_token::Column::UserId.eq(user_id))
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
    ) -> Result<email_verification_token::Model, AppError> {
        let model = email_verification_token::ActiveModel {
            id: sea_orm::NotSet,
            user_id: Set(user_id),
            token_hash: Set(token_hash),
            created_at: Set(created_at),
            expires_at: Set(expires_at),
            used: Set(false),
        };
        Ok(model.insert(self.db.as_ref()).await?)
    }

    async fn mark_as_used(&self, id: i32) -> Result<bool, AppError> {
        let result = email_verification_token::Entity::update_many()
            .filter(email_verification_token::Column::Id.eq(id))
            .filter(email_verification_token::Column::Used.eq(false))
            .set(email_verification_token::ActiveModel {
                used: Set(true),
                ..Default::default()
            })
            .exec(self.db.as_ref())
            .await?;
        Ok(result.rows_affected == 1)
    }
}
