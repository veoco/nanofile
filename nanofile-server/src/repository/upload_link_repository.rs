use async_trait::async_trait;
use sea_orm::{
    ColumnTrait, DatabaseConnection, DeleteResult, EntityTrait, QueryFilter,
};
use std::sync::Arc;

use crate::entity::upload_link;
use crate::error::AppError;

#[async_trait]
pub trait UploadLinkRepository: Send + Sync {
    async fn find_by_creator_id(
        &self,
        creator_id: i32,
    ) -> Result<Vec<upload_link::Model>, AppError>;
    async fn delete_by_token_and_user(
        &self,
        token: &str,
        user_id: i32,
    ) -> Result<DeleteResult, AppError>;
    async fn delete_by_id_and_user(
        &self,
        id: i32,
        user_id: i32,
    ) -> Result<DeleteResult, AppError>;
}

pub struct DbUploadLinkRepository {
    db: Arc<DatabaseConnection>,
}

impl DbUploadLinkRepository {
    pub fn new(db: Arc<DatabaseConnection>) -> Self {
        Self { db }
    }
}

#[async_trait]
impl UploadLinkRepository for DbUploadLinkRepository {
    async fn find_by_creator_id(
        &self,
        creator_id: i32,
    ) -> Result<Vec<upload_link::Model>, AppError> {
        Ok(upload_link::Entity::find()
            .filter(upload_link::Column::CreatorId.eq(creator_id))
            .all(self.db.as_ref())
            .await?)
    }

    async fn delete_by_token_and_user(
        &self,
        token: &str,
        user_id: i32,
    ) -> Result<DeleteResult, AppError> {
        Ok(upload_link::Entity::delete_many()
            .filter(upload_link::Column::Token.eq(token))
            .filter(upload_link::Column::CreatorId.eq(user_id))
            .exec(self.db.as_ref())
            .await?)
    }

    async fn delete_by_id_and_user(
        &self,
        id: i32,
        user_id: i32,
    ) -> Result<DeleteResult, AppError> {
        Ok(upload_link::Entity::delete_many()
            .filter(upload_link::Column::Id.eq(id))
            .filter(upload_link::Column::CreatorId.eq(user_id))
            .exec(self.db.as_ref())
            .await?)
    }
}
