use async_trait::async_trait;
use sea_orm::{ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter};
use std::sync::Arc;

use base::error::AppError;
use infra::entity::file_lock_timestamp;

#[async_trait]
pub trait FileLockTimestampRepository: Send + Sync {
    async fn find_by_repo(
        &self,
        repo_id: &str,
    ) -> Result<Option<file_lock_timestamp::Model>, AppError>;
    async fn delete_by_repo(&self, repo_id: &str) -> Result<(), AppError>;
}

pub struct DbFileLockTimestampRepository {
    db: Arc<DatabaseConnection>,
}

impl DbFileLockTimestampRepository {
    pub fn new(db: Arc<DatabaseConnection>) -> Self {
        Self { db }
    }
}

#[async_trait]
impl FileLockTimestampRepository for DbFileLockTimestampRepository {
    async fn find_by_repo(
        &self,
        repo_id: &str,
    ) -> Result<Option<file_lock_timestamp::Model>, AppError> {
        Ok(file_lock_timestamp::Entity::find()
            .filter(file_lock_timestamp::Column::RepoId.eq(repo_id))
            .one(self.db.as_ref())
            .await?)
    }

    async fn delete_by_repo(&self, repo_id: &str) -> Result<(), AppError> {
        file_lock_timestamp::Entity::delete_many()
            .filter(file_lock_timestamp::Column::RepoId.eq(repo_id))
            .exec(self.db.as_ref())
            .await?;
        Ok(())
    }
}
