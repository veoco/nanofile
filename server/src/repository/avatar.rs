use async_trait::async_trait;
use sea_orm::{ActiveModelTrait, ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter, Set};
use std::sync::Arc;

use crate::entity::avatar;
use crate::error::AppError;

#[async_trait]
pub trait AvatarRepository: Send + Sync {
    async fn find_by_email(&self, email: &str) -> Result<Option<avatar::Model>, AppError>;
    async fn upsert(
        &self,
        email: &str,
        file_name: &str,
        mime_type: &str,
        file_size: i32,
        now: i64,
    ) -> Result<(), AppError>;
}

pub struct DbAvatarRepository {
    db: Arc<DatabaseConnection>,
}

impl DbAvatarRepository {
    pub fn new(db: Arc<DatabaseConnection>) -> Self {
        Self { db }
    }
}

#[async_trait]
impl AvatarRepository for DbAvatarRepository {
    async fn find_by_email(&self, email: &str) -> Result<Option<avatar::Model>, AppError> {
        Ok(avatar::Entity::find()
            .filter(avatar::Column::Email.eq(email))
            .one(self.db.as_ref())
            .await?)
    }

    async fn upsert(
        &self,
        email: &str,
        file_name: &str,
        mime_type: &str,
        file_size: i32,
        now: i64,
    ) -> Result<(), AppError> {
        let existing = avatar::Entity::find()
            .filter(avatar::Column::Email.eq(email))
            .one(self.db.as_ref())
            .await?;

        if let Some(record) = existing {
            let mut active: avatar::ActiveModel = record.into();
            active.avatar_file_name = Set(file_name.to_string());
            active.mime_type = Set(mime_type.to_string());
            active.file_size = Set(file_size);
            active.date_uploaded = Set(now);
            active.update(self.db.as_ref()).await?;
        } else {
            avatar::ActiveModel {
                id: sea_orm::NotSet,
                email: Set(email.to_string()),
                avatar_file_name: Set(file_name.to_string()),
                mime_type: Set(mime_type.to_string()),
                file_size: Set(file_size),
                date_uploaded: Set(now),
            }
            .insert(self.db.as_ref())
            .await?;
        }
        Ok(())
    }
}
