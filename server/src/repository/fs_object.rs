use async_trait::async_trait;
use sea_orm::{ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter};
use std::sync::Arc;

use base::error::AppError;
use infra::entity::fs_object;

#[async_trait]
pub trait FsObjectRepository: Send + Sync {
    async fn find_by_repo_and_fs_id(
        &self,
        repo_id: &str,
        fs_id: &str,
    ) -> Result<Option<fs_object::Model>, AppError>;
    async fn exists_by_repo_and_fs_id(&self, repo_id: &str, fs_id: &str) -> Result<bool, AppError>;
    async fn find_by_repo_and_fs_ids(
        &self,
        repo_id: &str,
        fs_ids: &[String],
    ) -> Result<Vec<fs_object::Model>, AppError>;
    async fn insert_many(&self, models: Vec<fs_object::ActiveModel>) -> Result<(), AppError>;
    async fn find_all(&self) -> Result<Vec<fs_object::Model>, AppError>;
    async fn find_by_fs_id(&self, fs_id: &str) -> Result<Option<fs_object::Model>, AppError>;
    async fn delete_many_by_ids(&self, ids: Vec<i64>) -> Result<(), AppError>;
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

    async fn find_by_repo_and_fs_ids(
        &self,
        repo_id: &str,
        fs_ids: &[String],
    ) -> Result<Vec<fs_object::Model>, AppError> {
        if fs_ids.is_empty() {
            return Ok(Vec::new());
        }
        Ok(fs_object::Entity::find()
            .filter(fs_object::Column::RepoId.eq(repo_id))
            .filter(fs_object::Column::FsId.is_in(fs_ids))
            .all(self.db.as_ref())
            .await?)
    }

    async fn insert_many(&self, models: Vec<fs_object::ActiveModel>) -> Result<(), AppError> {
        if models.is_empty() {
            return Ok(());
        }
        fs_object::Entity::insert_many(models)
            .on_conflict(
                sea_orm::sea_query::OnConflict::columns([
                    fs_object::Column::RepoId,
                    fs_object::Column::FsId,
                ])
                .do_nothing()
                .to_owned(),
            )
            .exec(self.db.as_ref())
            .await?;
        Ok(())
    }

    async fn find_all(&self) -> Result<Vec<fs_object::Model>, AppError> {
        Ok(fs_object::Entity::find()
            .filter(fs_object::Column::RepoId.ne(""))
            .all(self.db.as_ref())
            .await?)
    }

    async fn find_by_fs_id(&self, fs_id: &str) -> Result<Option<fs_object::Model>, AppError> {
        Ok(fs_object::Entity::find()
            .filter(fs_object::Column::FsId.eq(fs_id))
            .one(self.db.as_ref())
            .await?)
    }

    async fn delete_many_by_ids(&self, ids: Vec<i64>) -> Result<(), AppError> {
        if ids.is_empty() {
            return Ok(());
        }
        fs_object::Entity::delete_many()
            .filter(fs_object::Column::Id.is_in(ids))
            .exec(self.db.as_ref())
            .await?;
        Ok(())
    }
}
