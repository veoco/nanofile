use async_trait::async_trait;
use sea_orm::{
    ActiveModelTrait, ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter, QueryOrder, Set,
};
use std::sync::Arc;

use crate::entity::user;
use crate::error::AppError;

#[async_trait]
pub trait UserRepository: Send + Sync {
    async fn find_by_id(&self, user_id: i32) -> Result<Option<user::Model>, AppError>;
    async fn find_by_email(&self, email: &str) -> Result<Option<user::Model>, AppError>;
    async fn find_by_email_like(&self, pattern: &str) -> Result<Vec<user::Model>, AppError>;
    async fn exists_by_email(&self, email: &str) -> Result<bool, AppError>;
    async fn create(&self, email: String, password_hash: String) -> Result<user::Model, AppError>;
    async fn create_with_params(
        &self,
        email: String,
        password_hash: String,
        is_admin: bool,
        is_active: bool,
        storage_quota: Option<i64>,
    ) -> Result<user::Model, AppError>;
    async fn update_display_name(&self, user_id: i32, name: Option<String>)
    -> Result<(), AppError>;
    async fn touch_last_login(&self, user_id: i32, now: i64) -> Result<(), AppError>;

    // Admin methods
    async fn find_all(&self) -> Result<Vec<user::Model>, AppError>;
    async fn update_is_admin(&self, user_id: i32, is_admin: bool) -> Result<(), AppError>;
    async fn update_is_active(&self, user_id: i32, is_active: bool) -> Result<(), AppError>;
    async fn update_storage_quota(&self, user_id: i32, quota: Option<i64>) -> Result<(), AppError>;
    async fn delete_user(&self, user_id: i32) -> Result<(), AppError>;

    // ── Methods for UI layer refactoring ───────────────────────────────
    /// Update the user's password hash.
    async fn update_password(&self, user_id: i32, password_hash: String) -> Result<(), AppError>;
    /// Create a new user with an inviter.
    async fn create_with_inviter(
        &self,
        email: String,
        password_hash: String,
        invited_by: Option<i32>,
    ) -> Result<user::Model, AppError>;
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

    async fn create(&self, email: String, password_hash: String) -> Result<user::Model, AppError> {
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
            storage_quota: sea_orm::NotSet,
            name: sea_orm::NotSet,
            display_name: sea_orm::NotSet,
        };
        let result = model.insert(self.db.as_ref()).await?;
        Ok(result)
    }

    async fn create_with_params(
        &self,
        email: String,
        password_hash: String,
        is_admin: bool,
        is_active: bool,
        storage_quota: Option<i64>,
    ) -> Result<user::Model, AppError> {
        let now = chrono::Utc::now().timestamp();
        let model = user::ActiveModel {
            id: sea_orm::NotSet,
            email: Set(email),
            password_hash: Set(password_hash),
            is_active: Set(is_active),
            is_admin: Set(is_admin),
            created_at: Set(now),
            last_login_at: sea_orm::NotSet,
            invited_by: Set(None),
            storage_quota: Set(storage_quota),
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

    async fn find_all(&self) -> Result<Vec<user::Model>, AppError> {
        Ok(user::Entity::find()
            .order_by_asc(user::Column::Id)
            .all(self.db.as_ref())
            .await?)
    }

    async fn update_is_admin(&self, user_id: i32, is_admin: bool) -> Result<(), AppError> {
        let user_record = self.find_by_id(user_id).await?;
        if let Some(u) = user_record {
            let mut active: user::ActiveModel = u.into();
            active.is_admin = Set(is_admin);
            active.update(self.db.as_ref()).await?;
        }
        Ok(())
    }

    async fn update_is_active(&self, user_id: i32, is_active: bool) -> Result<(), AppError> {
        let user_record = self.find_by_id(user_id).await?;
        if let Some(u) = user_record {
            let mut active: user::ActiveModel = u.into();
            active.is_active = Set(is_active);
            active.update(self.db.as_ref()).await?;
        }
        Ok(())
    }

    async fn update_storage_quota(&self, user_id: i32, quota: Option<i64>) -> Result<(), AppError> {
        let user_record = self.find_by_id(user_id).await?;
        if let Some(u) = user_record {
            let mut active: user::ActiveModel = u.into();
            active.storage_quota = Set(quota);
            active.update(self.db.as_ref()).await?;
        }
        Ok(())
    }

    async fn delete_user(&self, user_id: i32) -> Result<(), AppError> {
        user::Entity::delete_by_id(user_id)
            .exec(self.db.as_ref())
            .await?;
        Ok(())
    }

    async fn update_password(&self, user_id: i32, password_hash: String) -> Result<(), AppError> {
        let user_record = self.find_by_id(user_id).await?;
        if let Some(u) = user_record {
            let mut active: user::ActiveModel = u.into();
            active.password_hash = Set(password_hash);
            active.update(self.db.as_ref()).await?;
        }
        Ok(())
    }

    async fn create_with_inviter(
        &self,
        email: String,
        password_hash: String,
        invited_by: Option<i32>,
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
            invited_by: Set(invited_by),
            storage_quota: sea_orm::NotSet,
            name: sea_orm::NotSet,
            display_name: sea_orm::NotSet,
        };
        let result = model.insert(self.db.as_ref()).await?;
        Ok(result)
    }
}
