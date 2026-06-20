use async_trait::async_trait;
use sea_orm::{ColumnTrait, DatabaseConnection, DeleteResult, EntityTrait, QueryFilter};
use std::sync::Arc;

use crate::entity::starred_file;
use crate::error::AppError;

#[async_trait]
pub trait StarredRepository: Send + Sync {
    async fn find_by_user_id(&self, user_id: i32) -> Result<Vec<starred_file::Model>, AppError>;
    async fn find_by_user_and_repo(
        &self,
        user_id: i32,
        repo_id: &str,
    ) -> Result<Vec<starred_file::Model>, AppError>;
    async fn find_by_user_repo_and_path(
        &self,
        user_id: i32,
        repo_id: &str,
        path: &str,
    ) -> Result<Option<starred_file::Model>, AppError>;
    async fn delete_by_user_repo_and_path(
        &self,
        user_id: i32,
        repo_id: &str,
        path: &str,
    ) -> Result<DeleteResult, AppError>;
}

pub struct DbStarredRepository {
    db: Arc<DatabaseConnection>,
}

impl DbStarredRepository {
    pub fn new(db: Arc<DatabaseConnection>) -> Self {
        Self { db }
    }
}

#[async_trait]
impl StarredRepository for DbStarredRepository {
    async fn find_by_user_id(&self, user_id: i32) -> Result<Vec<starred_file::Model>, AppError> {
        Ok(starred_file::Entity::find()
            .filter(starred_file::Column::UserId.eq(user_id))
            .all(self.db.as_ref())
            .await?)
    }

    async fn find_by_user_and_repo(
        &self,
        user_id: i32,
        repo_id: &str,
    ) -> Result<Vec<starred_file::Model>, AppError> {
        Ok(starred_file::Entity::find()
            .filter(starred_file::Column::UserId.eq(user_id))
            .filter(starred_file::Column::RepoId.eq(repo_id))
            .all(self.db.as_ref())
            .await?)
    }

    async fn find_by_user_repo_and_path(
        &self,
        user_id: i32,
        repo_id: &str,
        path: &str,
    ) -> Result<Option<starred_file::Model>, AppError> {
        Ok(starred_file::Entity::find()
            .filter(starred_file::Column::UserId.eq(user_id))
            .filter(starred_file::Column::RepoId.eq(repo_id))
            .filter(starred_file::Column::Path.eq(path))
            .one(self.db.as_ref())
            .await?)
    }

    async fn delete_by_user_repo_and_path(
        &self,
        user_id: i32,
        repo_id: &str,
        path: &str,
    ) -> Result<DeleteResult, AppError> {
        Ok(starred_file::Entity::delete_many()
            .filter(starred_file::Column::UserId.eq(user_id))
            .filter(starred_file::Column::RepoId.eq(repo_id))
            .filter(starred_file::Column::Path.eq(path))
            .exec(self.db.as_ref())
            .await?)
    }
}
