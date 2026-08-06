use async_trait::async_trait;
use sea_orm::{
    ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter, Set, sea_query::OnConflict,
};
use std::sync::Arc;

use base::error::AppError;
use infra::entity::avatar;

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
        // Single `INSERT ... ON CONFLICT(email) DO UPDATE` avoids the
        // read-then-write race between concurrent avatar uploads.
        avatar::Entity::insert(avatar::ActiveModel {
            email: Set(email.to_string()),
            avatar_file_name: Set(file_name.to_string()),
            mime_type: Set(mime_type.to_string()),
            file_size: Set(file_size),
            date_uploaded: Set(now),
            ..Default::default()
        })
        .on_conflict(
            OnConflict::column(avatar::Column::Email)
                .update_columns([
                    avatar::Column::AvatarFileName,
                    avatar::Column::MimeType,
                    avatar::Column::FileSize,
                    avatar::Column::DateUploaded,
                ])
                .to_owned(),
        )
        .exec(self.db.as_ref())
        .await?;
        Ok(())
    }
}
