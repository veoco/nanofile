use async_trait::async_trait;
use sea_orm::{
    ActiveModelTrait, ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter, QueryOrder,
};
use std::sync::Arc;

use crate::entity::deleted_repo;
use crate::error::AppError;

#[async_trait]
pub trait DeletedRepoRepository: Send + Sync {
    async fn find_by_id(&self, repo_id: &str) -> Result<Option<deleted_repo::Model>, AppError>;
    async fn find_by_owner(&self, owner_id: i32) -> Result<Vec<deleted_repo::Model>, AppError>;
    async fn delete_by_id(&self, repo_id: &str) -> Result<(), AppError>;
    async fn insert(&self, model: deleted_repo::ActiveModel) -> Result<(), AppError>;
}

pub struct DbDeletedRepoRepository {
    db: Arc<DatabaseConnection>,
}

impl DbDeletedRepoRepository {
    pub fn new(db: Arc<DatabaseConnection>) -> Self {
        Self { db }
    }
}

#[async_trait]
impl DeletedRepoRepository for DbDeletedRepoRepository {
    async fn find_by_id(&self, repo_id: &str) -> Result<Option<deleted_repo::Model>, AppError> {
        Ok(deleted_repo::Entity::find_by_id(repo_id)
            .one(self.db.as_ref())
            .await?)
    }

    async fn find_by_owner(&self, owner_id: i32) -> Result<Vec<deleted_repo::Model>, AppError> {
        Ok(deleted_repo::Entity::find()
            .filter(deleted_repo::Column::OwnerId.eq(owner_id))
            .order_by_desc(deleted_repo::Column::DelTime)
            .all(self.db.as_ref())
            .await?)
    }

    async fn delete_by_id(&self, repo_id: &str) -> Result<(), AppError> {
        deleted_repo::Entity::delete_by_id(repo_id)
            .exec(self.db.as_ref())
            .await?;
        Ok(())
    }

    async fn insert(&self, model: deleted_repo::ActiveModel) -> Result<(), AppError> {
        model.insert(self.db.as_ref()).await?;
        Ok(())
    }
}
