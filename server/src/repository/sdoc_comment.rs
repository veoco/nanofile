use async_trait::async_trait;
use sea_orm::{ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter, QueryOrder};
use std::sync::Arc;

use crate::entity::sdoc_comment;
use crate::error::AppError;

#[async_trait]
pub trait SdocCommentRepository: Send + Sync {
    async fn find_by_doc_uuid(&self, doc_uuid: &str) -> Result<Vec<sdoc_comment::Model>, AppError>;
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

    async fn delete_by_id(&self, id: i32) -> Result<(), AppError> {
        sdoc_comment::Entity::delete_by_id(id)
            .exec(self.db.as_ref())
            .await?;
        Ok(())
    }
}
