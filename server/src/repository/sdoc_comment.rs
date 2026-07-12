use async_trait::async_trait;
use sea_orm::{
    ActiveModelTrait, ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter, QueryOrder, Set,
};
use std::sync::Arc;

use crate::entity::sdoc_comment;
use crate::error::AppError;

#[async_trait]
pub trait SdocCommentRepository: Send + Sync {
    async fn find_by_doc_uuid(&self, doc_uuid: &str) -> Result<Vec<sdoc_comment::Model>, AppError>;
    async fn find_by_id(&self, id: i32) -> Result<Option<sdoc_comment::Model>, AppError>;
    async fn create(
        &self,
        doc_uuid: &str,
        user_id: i32,
        content: &str,
    ) -> Result<sdoc_comment::Model, AppError>;
    async fn update_resolved(&self, id: i32, resolved: bool) -> Result<(), AppError>;
    async fn delete_by_id(&self, id: i32) -> Result<(), AppError>;
}

pub struct DbSdocCommentRepository {
    db: Arc<DatabaseConnection>,
}

impl DbSdocCommentRepository {
    pub fn new(db: Arc<DatabaseConnection>) -> Self {
        Self { db }
    }
}

#[async_trait]
impl SdocCommentRepository for DbSdocCommentRepository {
    async fn find_by_doc_uuid(&self, doc_uuid: &str) -> Result<Vec<sdoc_comment::Model>, AppError> {
        Ok(sdoc_comment::Entity::find()
            .filter(sdoc_comment::Column::DocUuid.eq(doc_uuid))
            .order_by_asc(sdoc_comment::Column::CreatedAt)
            .all(self.db.as_ref())
            .await?)
    }

    async fn find_by_id(&self, id: i32) -> Result<Option<sdoc_comment::Model>, AppError> {
        Ok(sdoc_comment::Entity::find_by_id(id)
            .one(self.db.as_ref())
            .await?)
    }

    async fn create(
        &self,
        doc_uuid: &str,
        user_id: i32,
        content: &str,
    ) -> Result<sdoc_comment::Model, AppError> {
        let now = chrono::Utc::now().timestamp();
        let active = sdoc_comment::ActiveModel {
            id: sea_orm::NotSet,
            doc_uuid: Set(doc_uuid.to_string()),
            user_id: Set(user_id),
            content: Set(content.to_string()),
            resolved: Set(Some(false)),
            created_at: Set(now),
        };
        let result = active.insert(self.db.as_ref()).await?;
        Ok(result)
    }

    async fn update_resolved(&self, id: i32, resolved: bool) -> Result<(), AppError> {
        let comment = sdoc_comment::Entity::find_by_id(id)
            .one(self.db.as_ref())
            .await?
            .ok_or_else(|| AppError::NotFound("comment not found".into()))?;
        let mut active: sdoc_comment::ActiveModel = comment.into();
        active.resolved = Set(Some(resolved));
        active.update(self.db.as_ref()).await?;
        Ok(())
    }

    async fn delete_by_id(&self, id: i32) -> Result<(), AppError> {
        sdoc_comment::Entity::delete_by_id(id)
            .exec(self.db.as_ref())
            .await?;
        Ok(())
    }
}
