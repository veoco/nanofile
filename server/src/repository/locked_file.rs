use async_trait::async_trait;
use sea_orm::{ActiveModelTrait, ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter, Set};
use std::sync::Arc;

use crate::entity::locked_file;
use crate::error::AppError;

#[async_trait]
pub trait LockedFileRepository: Send + Sync {
    async fn find_by_repo_and_path(
        &self,
        repo_id: &str,
        path: &str,
    ) -> Result<Option<locked_file::Model>, AppError>;
    async fn find_by_repo(&self, repo_id: &str) -> Result<Vec<locked_file::Model>, AppError>;
    async fn create(
        &self,
        repo_id: &str,
        path: &str,
        user_id: i32,
        now: i64,
        owner_name: &str,
    ) -> Result<(), AppError>;
    async fn delete_by_repo_and_path(&self, repo_id: &str, path: &str) -> Result<(), AppError>;
    async fn update(&self, model: locked_file::ActiveModel) -> Result<(), AppError>;
}

pub struct DbLockedFileRepository {
    db: Arc<DatabaseConnection>,
}

impl DbLockedFileRepository {
    pub fn new(db: Arc<DatabaseConnection>) -> Self {
        Self { db }
    }
}

#[async_trait]
impl LockedFileRepository for DbLockedFileRepository {
    async fn find_by_repo_and_path(
        &self,
        repo_id: &str,
        path: &str,
    ) -> Result<Option<locked_file::Model>, AppError> {
        Ok(locked_file::Entity::find()
            .filter(locked_file::Column::RepoId.eq(repo_id))
            .filter(locked_file::Column::Path.eq(path))
            .one(self.db.as_ref())
            .await?)
    }

    async fn find_by_repo(&self, repo_id: &str) -> Result<Vec<locked_file::Model>, AppError> {
        Ok(locked_file::Entity::find()
            .filter(locked_file::Column::RepoId.eq(repo_id))
            .all(self.db.as_ref())
            .await?)
    }

    async fn create(
        &self,
        repo_id: &str,
        path: &str,
        user_id: i32,
        now: i64,
        owner_name: &str,
    ) -> Result<(), AppError> {
        locked_file::Entity::insert(locked_file::ActiveModel {
            id: sea_orm::NotSet,
            repo_id: Set(repo_id.to_string()),
            path: Set(path.to_string()),
            user_id: Set(user_id),
            locked_at: Set(now),
            lock_owner_name: Set(owner_name.to_string()),
        })
        .exec(self.db.as_ref())
        .await?;
        Ok(())
    }

    async fn delete_by_repo_and_path(&self, repo_id: &str, path: &str) -> Result<(), AppError> {
        locked_file::Entity::delete_many()
            .filter(locked_file::Column::RepoId.eq(repo_id))
            .filter(locked_file::Column::Path.eq(path))
            .exec(self.db.as_ref())
            .await?;
        Ok(())
    }

    async fn update(&self, model: locked_file::ActiveModel) -> Result<(), AppError> {
        model.update(self.db.as_ref()).await?;
        Ok(())
    }
}
