use async_trait::async_trait;
use sea_orm::{
    ActiveModelTrait, ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter, QueryOrder,
    QuerySelect, Set,
};
use std::sync::Arc;

use base::error::AppError;
use infra::entity::user;

#[async_trait]
pub trait UserRepository: Send + Sync {
    async fn find_by_id(&self, user_id: i32) -> Result<Option<user::Model>, AppError>;
    /// Fetch several users by id in one query (for list pages with N+1 lookups).
    async fn find_by_ids(&self, user_ids: &[i32]) -> Result<Vec<user::Model>, AppError>;
    async fn find_by_email(&self, email: &str) -> Result<Option<user::Model>, AppError>;
    /// Fetch several users by email in one query (for directory listings).
    async fn find_by_emails(&self, emails: &[String]) -> Result<Vec<user::Model>, AppError>;
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
    async fn update_language(&self, user_id: i32, language: Option<String>)
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

    async fn find_by_ids(&self, user_ids: &[i32]) -> Result<Vec<user::Model>, AppError> {
        if user_ids.is_empty() {
            return Ok(Vec::new());
        }
        // SQLite has a ~999 bound on bound parameters; chunk the IN list.
        const IN_BATCH: usize = 500;
        let mut out = Vec::new();
        for chunk in user_ids.chunks(IN_BATCH) {
            let rows = user::Entity::find()
                .filter(user::Column::Id.is_in(chunk.iter().cloned()))
                .all(self.db.as_ref())
                .await?;
            out.extend(rows);
        }
        Ok(out)
    }

    async fn find_by_email(&self, email: &str) -> Result<Option<user::Model>, AppError> {
        Ok(user::Entity::find()
            .filter(user::Column::Email.eq(email))
            .one(self.db.as_ref())
            .await?)
    }

    async fn find_by_emails(&self, emails: &[String]) -> Result<Vec<user::Model>, AppError> {
        if emails.is_empty() {
            return Ok(Vec::new());
        }
        // SQLite has a ~999 bound on bound parameters; chunk the IN list.
        const IN_BATCH: usize = 500;
        let mut out = Vec::new();
        for chunk in emails.chunks(IN_BATCH) {
            let rows = user::Entity::find()
                .filter(user::Column::Email.is_in(chunk.iter().cloned()))
                .all(self.db.as_ref())
                .await?;
            out.extend(rows);
        }
        Ok(out)
    }

    async fn find_by_email_like(&self, pattern: &str) -> Result<Vec<user::Model>, AppError> {
        Ok(user::Entity::find()
            .filter(user::Column::Email.like(pattern))
            .all(self.db.as_ref())
            .await?)
    }

    async fn exists_by_email(&self, email: &str) -> Result<bool, AppError> {
        // Project only the primary key — avoid pulling `password_hash` and the
        // rest of the row just to check for existence.
        let row: Option<(i32,)> = user::Entity::find()
            .select_only()
            .column(user::Column::Id)
            .filter(user::Column::Email.eq(email))
            .into_tuple()
            .one(self.db.as_ref())
            .await?;
        Ok(row.is_some())
    }

    async fn create(&self, email: String, password_hash: String) -> Result<user::Model, AppError> {
        self.create_with_params(email, password_hash, false, true, None)
            .await
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
            email: Set(email),
            password_hash: Set(password_hash),
            is_active: Set(is_active),
            is_admin: Set(is_admin),
            created_at: Set(now),
            storage_quota: Set(storage_quota),
            ..Default::default()
        };
        let result = model.insert(self.db.as_ref()).await?;
        Ok(result)
    }

    async fn update_display_name(
        &self,
        user_id: i32,
        name: Option<String>,
    ) -> Result<(), AppError> {
        user::Entity::update_many()
            .filter(user::Column::Id.eq(user_id))
            .set(user::ActiveModel {
                display_name: Set(name),
                ..Default::default()
            })
            .exec(self.db.as_ref())
            .await?;
        Ok(())
    }

    async fn update_language(
        &self,
        user_id: i32,
        language: Option<String>,
    ) -> Result<(), AppError> {
        user::Entity::update_many()
            .filter(user::Column::Id.eq(user_id))
            .set(user::ActiveModel {
                language: Set(language),
                ..Default::default()
            })
            .exec(self.db.as_ref())
            .await?;
        Ok(())
    }

    async fn touch_last_login(&self, user_id: i32, now: i64) -> Result<(), AppError> {
        user::Entity::update_many()
            .filter(user::Column::Id.eq(user_id))
            .set(user::ActiveModel {
                last_login_at: Set(Some(now)),
                ..Default::default()
            })
            .exec(self.db.as_ref())
            .await?;
        Ok(())
    }

    async fn find_all(&self) -> Result<Vec<user::Model>, AppError> {
        Ok(user::Entity::find()
            .order_by_asc(user::Column::Id)
            .all(self.db.as_ref())
            .await?)
    }

    async fn update_is_admin(&self, user_id: i32, is_admin: bool) -> Result<(), AppError> {
        user::Entity::update_many()
            .filter(user::Column::Id.eq(user_id))
            .set(user::ActiveModel {
                is_admin: Set(is_admin),
                ..Default::default()
            })
            .exec(self.db.as_ref())
            .await?;
        Ok(())
    }

    async fn update_is_active(&self, user_id: i32, is_active: bool) -> Result<(), AppError> {
        user::Entity::update_many()
            .filter(user::Column::Id.eq(user_id))
            .set(user::ActiveModel {
                is_active: Set(is_active),
                ..Default::default()
            })
            .exec(self.db.as_ref())
            .await?;
        Ok(())
    }

    async fn update_storage_quota(&self, user_id: i32, quota: Option<i64>) -> Result<(), AppError> {
        user::Entity::update_many()
            .filter(user::Column::Id.eq(user_id))
            .set(user::ActiveModel {
                storage_quota: Set(quota),
                ..Default::default()
            })
            .exec(self.db.as_ref())
            .await?;
        Ok(())
    }

    async fn delete_user(&self, user_id: i32) -> Result<(), AppError> {
        user::Entity::delete_by_id(user_id)
            .exec(self.db.as_ref())
            .await?;
        Ok(())
    }

    async fn update_password(&self, user_id: i32, password_hash: String) -> Result<(), AppError> {
        user::Entity::update_many()
            .filter(user::Column::Id.eq(user_id))
            .set(user::ActiveModel {
                password_hash: Set(password_hash),
                ..Default::default()
            })
            .exec(self.db.as_ref())
            .await?;
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
            email: Set(email),
            password_hash: Set(password_hash),
            is_active: Set(true),
            is_admin: Set(false),
            created_at: Set(now),
            invited_by: Set(invited_by),
            ..Default::default()
        };
        let result = model.insert(self.db.as_ref()).await?;
        Ok(result)
    }
}
