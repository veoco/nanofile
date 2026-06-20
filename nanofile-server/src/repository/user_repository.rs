use async_trait::async_trait;
use sea_orm::{ActiveModelTrait, ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter, Set};
use std::sync::Arc;

use crate::entity::user;
use crate::error::AppError;

#[async_trait]
pub trait UserRepository: Send + Sync {
    async fn find_by_id(&self, user_id: i32) -> Result<Option<user::Model>, AppError>;
    async fn find_by_email(&self, email: &str) -> Result<Option<user::Model>, AppError>;
    async fn find_by_email_like(&self, pattern: &str) -> Result<Vec<user::Model>, AppError>;
    async fn exists_by_email(&self, email: &str) -> Result<bool, AppError>;
    async fn create(
        &self,
        email: String,
        password_hash: String,
    ) -> Result<user::Model, AppError>;
    async fn update_display_name(
        &self,
        user_id: i32,
        name: Option<String>,
    ) -> Result<(), AppError>;
    async fn touch_last_login(&self, user_id: i32, now: i64) -> Result<(), AppError>;
}

pub struct DbUserRepository {
    db: Arc<DatabaseConnection>,
}

impl DbUserRepository {
    pub fn new(db: Arc<DatabaseConnection>) -> Self {
        Self { db }
    }
}

#[async_trait]
impl UserRepository for DbUserRepository {
    async fn find_by_id(&self, user_id: i32) -> Result<Option<user::Model>, AppError> {
        Ok(user::Entity::find_by_id(user_id)
            .one(self.db.as_ref())
            .await?)
    }

    async fn find_by_email(&self, email: &str) -> Result<Option<user::Model>, AppError> {
        Ok(user::Entity::find()
            .filter(user::Column::Email.eq(email))
            .one(self.db.as_ref())
            .await?)
    }

    async fn find_by_email_like(&self, pattern: &str) -> Result<Vec<user::Model>, AppError> {
        Ok(user::Entity::find()
            .filter(user::Column::Email.like(pattern))
            .all(self.db.as_ref())
            .await?)
    }

    async fn exists_by_email(&self, email: &str) -> Result<bool, AppError> {
        Ok(user::Entity::find()
            .filter(user::Column::Email.eq(email))
            .one(self.db.as_ref())
            .await?
            .is_some())
    }

    async fn create(
        &self,
        email: String,
        password_hash: String,
    ) -> Result<user::Model, AppError> {
        let now = chrono::Utc::now().timestamp();
        let model = user::ActiveModel {
            id: sea_orm::NotSet,
            email: Set(email),
            password_hash: Set(password_hash),
            is_active: Set(true),
            is_admin: Set(false),
            created_at: Set(now),
            last_login_at: sea_orm::NotSet,
            invited_by: Set(None),
            name: sea_orm::NotSet,
            display_name: sea_orm::NotSet,
        };
        let result = model.insert(self.db.as_ref()).await?;
        Ok(result)
    }

    async fn update_display_name(
        &self,
        user_id: i32,
        name: Option<String>,
    ) -> Result<(), AppError> {
        let user_record = self.find_by_id(user_id).await?;
        if let Some(u) = user_record {
            let mut active: user::ActiveModel = u.into();
            active.display_name = Set(name);
            active.update(self.db.as_ref()).await?;
        }
        Ok(())
    }

    async fn touch_last_login(&self, user_id: i32, now: i64) -> Result<(), AppError> {
        let user_record = self.find_by_id(user_id).await?;
        if let Some(u) = user_record {
            let mut active: user::ActiveModel = u.into();
            active.last_login_at = Set(Some(now));
            active.update(self.db.as_ref()).await?;
        }
        Ok(())
    }
}
