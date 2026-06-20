use async_trait::async_trait;
use sea_orm::{ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter};
use std::sync::Arc;

use crate::entity::fs_object;
use crate::error::AppError;

#[async_trait]
pub trait FsObjectRepository: Send + Sync {
    async fn find_by_repo_and_fs_id(
        &self,
        repo_id: &str,
        fs_id: &str,
    ) -> Result<Option<fs_object::Model>, AppError>;
    async fn exists_by_repo_and_fs_id(&self, repo_id: &str, fs_id: &str) -> Result<bool, AppError>;
}

pub struct DbFsObjectRepository {
    db: Arc<DatabaseConnection>,
}

impl DbFsObjectRepository {
    pub fn new(db: Arc<DatabaseConnection>) -> Self {
        Self { db }
    }
}

#[async_trait]
impl FsObjectRepository for DbFsObjectRepository {
    async fn find_by_repo_and_fs_id(
        &self,
        repo_id: &str,
        fs_id: &str,
    ) -> Result<Option<fs_object::Model>, AppError> {
        Ok(fs_object::Entity::find()
            .filter(fs_object::Column::RepoId.eq(repo_id))
            .filter(fs_object::Column::FsId.eq(fs_id))
            .one(self.db.as_ref())
            .await?)
    }

    async fn exists_by_repo_and_fs_id(&self, repo_id: &str, fs_id: &str) -> Result<bool, AppError> {
        Ok(fs_object::Entity::find()
            .filter(fs_object::Column::RepoId.eq(repo_id))
            .filter(fs_object::Column::FsId.eq(fs_id))
            .one(self.db.as_ref())
            .await?
            .is_some())
    }
}
