use async_trait::async_trait;
use sea_orm::{ActiveModelTrait, ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter};
use std::sync::Arc;

use crate::entity::repo;
use crate::error::AppError;

#[async_trait]
pub trait RepoRepository: Send + Sync {
    async fn find_by_id(&self, repo_id: &str) -> Result<Option<repo::Model>, AppError>;
    async fn find_by_owner_id(&self, user_id: i32) -> Result<Vec<repo::Model>, AppError>;
    async fn create(&self, model: repo::ActiveModel) -> Result<repo::Model, AppError>;
    async fn update(&self, model: repo::ActiveModel) -> Result<repo::Model, AppError>;
    async fn delete_by_id(&self, repo_id: &str) -> Result<(), AppError>;
}

pub struct DbRepoRepository {
    db: Arc<DatabaseConnection>,
}

impl DbRepoRepository {
    pub fn new(db: Arc<DatabaseConnection>) -> Self {
        Self { db }
    }
}

#[async_trait]
impl RepoRepository for DbRepoRepository {
    async fn find_by_id(&self, repo_id: &str) -> Result<Option<repo::Model>, AppError> {
        Ok(repo::Entity::find_by_id(repo_id)
            .one(self.db.as_ref())
            .await?)
    }

    async fn find_by_owner_id(&self, user_id: i32) -> Result<Vec<repo::Model>, AppError> {
        Ok(repo::Entity::find()
            .filter(repo::Column::OwnerId.eq(user_id))
            .all(self.db.as_ref())
            .await?)
    }

    async fn create(&self, model: repo::ActiveModel) -> Result<repo::Model, AppError> {
        // Extract the repo_id before insert (ActiveModel will be consumed)
        let repo_id = match &model.id {
            sea_orm::Set(id) => id.clone(),
            _ => return Err(AppError::Internal("repo id is required".into())),
        };
        repo::Entity::insert(model).exec(self.db.as_ref()).await?;
        self.find_by_id(&repo_id)
            .await?
            .ok_or_else(|| AppError::Internal("Failed to find created repo".into()))
    }

    async fn update(&self, model: repo::ActiveModel) -> Result<repo::Model, AppError> {
        let result = model.update(self.db.as_ref()).await?;
        Ok(result)
    }

    async fn delete_by_id(&self, repo_id: &str) -> Result<(), AppError> {
        repo::Entity::delete_by_id(repo_id)
            .exec(self.db.as_ref())
            .await?;
        Ok(())
    }
}
