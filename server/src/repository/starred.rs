use async_trait::async_trait;
use sea_orm::{
    ActiveModelTrait, ColumnTrait, ConnectionTrait, DatabaseConnection, DeleteResult, EntityTrait,
    QueryFilter, Set, Statement,
};
use std::sync::Arc;

use base::error::AppError;
use infra::entity::starred_file;

/// Parameters for creating a starred file entry.
pub struct CreateStarredParams {
    pub repo_id: String,
    pub path: String,
    pub user_id: i32,
    pub is_dir: bool,
    pub created_at: i64,
}

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
    async fn insert(&self, model: starred_file::ActiveModel) -> Result<(), AppError>;
    /// Create a starred file entry from typed parameters.
    async fn create_starred(&self, params: CreateStarredParams) -> Result<(), AppError>;

    /// After a file/dir rename, update all starred entries that had the old
    /// path prefix to reflect the new path.
    async fn update_paths_for_rename(
        &self,
        old_path: &str,
        new_path: &str,
        repo_id: &str,
    ) -> Result<(), AppError>;
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
    async fn create_starred(&self, params: CreateStarredParams) -> Result<(), AppError> {
        let model = starred_file::ActiveModel {
            id: sea_orm::NotSet,
            repo_id: Set(params.repo_id),
            path: Set(params.path),
            user_id: Set(params.user_id),
            is_dir: Set(params.is_dir),
            created_at: Set(params.created_at),
        };
        model.insert(self.db.as_ref()).await?;
        Ok(())
    }
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

    async fn insert(&self, model: starred_file::ActiveModel) -> Result<(), AppError> {
        model.insert(self.db.as_ref()).await?;
        Ok(())
    }

    async fn update_paths_for_rename(
        &self,
        old_path: &str,
        new_path: &str,
        repo_id: &str,
    ) -> Result<(), AppError> {
        self.db
            .as_ref()
            .execute(Statement::from_sql_and_values(
                self.db.as_ref().get_database_backend(),
                "UPDATE starred_files SET path = $1 || substr(path, length($2) + 1) \
                 WHERE repo_id = $3 AND (path = $2 OR path LIKE $2 || '/%')",
                [new_path.into(), old_path.into(), repo_id.into()],
            ))
            .await?;
        Ok(())
    }
}
