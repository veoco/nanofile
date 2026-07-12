use async_trait::async_trait;
use sea_orm::{
    ActiveModelTrait, ColumnTrait, DatabaseConnection, EntityTrait, Order, PaginatorTrait,
    QueryFilter, QueryOrder, QuerySelect,
};
use std::sync::Arc;

use base::error::AppError;
use infra::entity::file_trash;

#[async_trait]
pub trait FileTrashRepository: Send + Sync {
    async fn insert(&self, model: file_trash::ActiveModel) -> Result<(), AppError>;
    async fn count_by_repo(&self, repo_id: &str) -> Result<i64, AppError>;
    async fn find_by_repo_paginated(
        &self,
        repo_id: &str,
        limit: u64,
        offset: u64,
    ) -> Result<Vec<file_trash::Model>, AppError>;
    async fn find_by_repo_cursor(
        &self,
        repo_id: &str,
        cursor: Option<i64>,
        limit: u64,
    ) -> Result<Vec<file_trash::Model>, AppError>;
    async fn find_by_compound_key(
        &self,
        repo_id: &str,
        commit_id: &str,
        path: &str,
        obj_name: &str,
    ) -> Result<Option<file_trash::Model>, AppError>;
    async fn delete_by_ids(&self, ids: &[i32]) -> Result<(), AppError>;
    async fn delete_by_repo(&self, repo_id: &str) -> Result<(), AppError>;
    async fn delete_by_repo_before(&self, repo_id: &str, cutoff: i64) -> Result<(), AppError>;
}

pub struct DbFileTrashRepository {
    db: Arc<DatabaseConnection>,
}

impl DbFileTrashRepository {
    pub fn new(db: Arc<DatabaseConnection>) -> Self {
        Self { db }
    }
}

#[async_trait]
impl FileTrashRepository for DbFileTrashRepository {
    async fn insert(&self, model: file_trash::ActiveModel) -> Result<(), AppError> {
        model.insert(self.db.as_ref()).await?;
        Ok(())
    }

    async fn count_by_repo(&self, repo_id: &str) -> Result<i64, AppError> {
        let count = file_trash::Entity::find()
            .filter(file_trash::Column::RepoId.eq(repo_id))
            .count(self.db.as_ref())
            .await?;
        Ok(count as i64)
    }

    async fn find_by_repo_paginated(
        &self,
        repo_id: &str,
        limit: u64,
        offset: u64,
    ) -> Result<Vec<file_trash::Model>, AppError> {
        Ok(file_trash::Entity::find()
            .filter(file_trash::Column::RepoId.eq(repo_id))
            .order_by(file_trash::Column::DeleteTime, Order::Desc)
            .limit(limit)
            .offset(offset)
            .all(self.db.as_ref())
            .await?)
    }

    async fn find_by_repo_cursor(
        &self,
        repo_id: &str,
        cursor: Option<i64>,
        limit: u64,
    ) -> Result<Vec<file_trash::Model>, AppError> {
        let mut query = file_trash::Entity::find()
            .filter(file_trash::Column::RepoId.eq(repo_id))
            .order_by(file_trash::Column::DeleteTime, Order::Desc)
            .limit(limit);
        if let Some(c) = cursor {
            query = query.filter(file_trash::Column::DeleteTime.lt(c));
        }
        Ok(query.all(self.db.as_ref()).await?)
    }

    async fn find_by_compound_key(
        &self,
        repo_id: &str,
        commit_id: &str,
        path: &str,
        obj_name: &str,
    ) -> Result<Option<file_trash::Model>, AppError> {
        Ok(file_trash::Entity::find()
            .filter(file_trash::Column::RepoId.eq(repo_id))
            .filter(file_trash::Column::CommitId.eq(commit_id))
            .filter(file_trash::Column::Path.eq(path))
            .filter(file_trash::Column::ObjName.eq(obj_name))
            .one(self.db.as_ref())
            .await?)
    }

    async fn delete_by_ids(&self, ids: &[i32]) -> Result<(), AppError> {
        if ids.is_empty() {
            return Ok(());
        }
        file_trash::Entity::delete_many()
            .filter(file_trash::Column::Id.is_in(ids.iter().copied()))
            .exec(self.db.as_ref())
            .await?;
        Ok(())
    }

    async fn delete_by_repo(&self, repo_id: &str) -> Result<(), AppError> {
        file_trash::Entity::delete_many()
            .filter(file_trash::Column::RepoId.eq(repo_id))
            .exec(self.db.as_ref())
            .await?;
        Ok(())
    }

    async fn delete_by_repo_before(&self, repo_id: &str, cutoff: i64) -> Result<(), AppError> {
        file_trash::Entity::delete_many()
            .filter(file_trash::Column::RepoId.eq(repo_id))
            .filter(file_trash::Column::DeleteTime.lt(cutoff))
            .exec(self.db.as_ref())
            .await?;
        Ok(())
    }
}
