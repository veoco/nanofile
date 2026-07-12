use async_trait::async_trait;
use sea_orm::{ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter, Set};
use std::sync::Arc;

use base::error::AppError;
use infra::entity::thumbnail;

#[async_trait]
pub trait ThumbnailRepository: Send + Sync {
    async fn find_by_repo_path_size(
        &self,
        repo_id: &str,
        path: &str,
        size: i32,
    ) -> Result<Option<thumbnail::Model>, AppError>;

    async fn create(
        &self,
        repo_id: &str,
        path: &str,
        size: i32,
        file_modified_at: i64,
        created_at: i64,
    ) -> Result<(), AppError>;

    async fn update_mtime(
        &self,
        repo_id: &str,
        path: &str,
        size: i32,
        file_modified_at: i64,
        now: i64,
    ) -> Result<(), AppError>;

    async fn delete_by_path(&self, repo_id: &str, path: &str) -> Result<(), AppError>;
}

pub struct DbThumbnailRepository {
    db: Arc<DatabaseConnection>,
}

impl DbThumbnailRepository {
    pub fn new(db: Arc<DatabaseConnection>) -> Self {
        Self { db }
    }
}

#[async_trait]
impl ThumbnailRepository for DbThumbnailRepository {
    async fn find_by_repo_path_size(
        &self,
        repo_id: &str,
        path: &str,
        size: i32,
    ) -> Result<Option<thumbnail::Model>, AppError> {
        Ok(thumbnail::Entity::find()
            .filter(thumbnail::Column::RepoId.eq(repo_id))
            .filter(thumbnail::Column::Path.eq(path))
            .filter(thumbnail::Column::Size.eq(size))
            .one(self.db.as_ref())
            .await?)
    }

    async fn create(
        &self,
        repo_id: &str,
        path: &str,
        size: i32,
        file_modified_at: i64,
        created_at: i64,
    ) -> Result<(), AppError> {
        thumbnail::Entity::insert(thumbnail::ActiveModel {
            id: sea_orm::NotSet,
            repo_id: Set(repo_id.to_string()),
            path: Set(path.to_string()),
            size: Set(size),
            file_modified_at: Set(file_modified_at),
            created_at: Set(created_at),
        })
        .exec(self.db.as_ref())
        .await?;
        Ok(())
    }

    async fn update_mtime(
        &self,
        repo_id: &str,
        path: &str,
        size: i32,
        file_modified_at: i64,
        now: i64,
    ) -> Result<(), AppError> {
        thumbnail::Entity::update_many()
            .filter(thumbnail::Column::RepoId.eq(repo_id))
            .filter(thumbnail::Column::Path.eq(path))
            .filter(thumbnail::Column::Size.eq(size))
            .set(thumbnail::ActiveModel {
                file_modified_at: Set(file_modified_at),
                created_at: Set(now),
                ..Default::default()
            })
            .exec(self.db.as_ref())
            .await?;
        Ok(())
    }

    async fn delete_by_path(&self, repo_id: &str, path: &str) -> Result<(), AppError> {
        thumbnail::Entity::delete_many()
            .filter(thumbnail::Column::RepoId.eq(repo_id))
            .filter(thumbnail::Column::Path.eq(path))
            .exec(self.db.as_ref())
            .await?;
        Ok(())
    }
}
