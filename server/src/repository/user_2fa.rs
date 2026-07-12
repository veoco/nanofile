use async_trait::async_trait;
use sea_orm::{ActiveModelTrait, ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter, Set};
use std::sync::Arc;

use crate::entity::user_2fa;
use crate::error::AppError;

#[async_trait]
pub trait User2faRepository: Send + Sync {
    async fn find_by_user_id(&self, user_id: i32) -> Result<Option<user_2fa::Model>, AppError>;
    async fn get_or_create(
        &self,
        user_id: i32,
        totp_secret: String,
    ) -> Result<user_2fa::Model, AppError>;
    async fn set_enabled(&self, user_id: i32, enabled: bool, now: i64) -> Result<(), AppError>;
    async fn delete_by_user_id(&self, user_id: i32) -> Result<(), AppError>;
}

pub struct DbUser2faRepository {
    db: Arc<DatabaseConnection>,
}

impl DbUser2faRepository {
    pub fn new(db: Arc<DatabaseConnection>) -> Self {
        Self { db }
    }
}

#[async_trait]
impl User2faRepository for DbUser2faRepository {
    async fn find_by_user_id(&self, user_id: i32) -> Result<Option<user_2fa::Model>, AppError> {
        Ok(user_2fa::Entity::find_by_id(user_id)
            .one(self.db.as_ref())
            .await?)
    }

    async fn get_or_create(
        &self,
        user_id: i32,
        totp_secret: String,
    ) -> Result<user_2fa::Model, AppError> {
        let existing = user_2fa::Entity::find_by_id(user_id)
            .one(self.db.as_ref())
            .await?;
        if let Some(model) = existing {
            Ok(model)
        } else {
            let model = user_2fa::ActiveModel {
                user_id: Set(user_id),
                totp_secret: Set(totp_secret),
                algorithm: Set("SHA1".to_string()),
                digits: Set(6i16),
                period: Set(30i16),
                enabled: Set(false),
                enabled_at: Set(None),
            };
            Ok(model.insert(self.db.as_ref()).await?)
        }
    }

    async fn set_enabled(&self, user_id: i32, enabled: bool, now: i64) -> Result<(), AppError> {
        let result = user_2fa::Entity::update_many()
            .filter(user_2fa::Column::UserId.eq(user_id))
            .set(user_2fa::ActiveModel {
                enabled: Set(enabled),
                enabled_at: Set(if enabled { Some(now) } else { None }),
                ..Default::default()
            })
            .exec(self.db.as_ref())
            .await?;
        if result.rows_affected == 0 {
            return Err(AppError::BadRequest("2FA not set up".into()));
        }
        Ok(())
    }

    async fn delete_by_user_id(&self, user_id: i32) -> Result<(), AppError> {
        user_2fa::Entity::delete_many()
            .filter(user_2fa::Column::UserId.eq(user_id))
            .exec(self.db.as_ref())
            .await?;
        Ok(())
    }
}
