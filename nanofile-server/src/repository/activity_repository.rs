use async_trait::async_trait;
use sea_orm::{ColumnTrait, DatabaseConnection, EntityTrait, PaginatorTrait, QueryFilter, QueryOrder, QuerySelect};
use std::sync::Arc;

use crate::entity::activity;
use crate::error::AppError;

#[async_trait]
pub trait ActivityRepository: Send + Sync {
    async fn find_by_repo_ids_filtered(
        &self,
        repo_ids: Vec<String>,
        user_id: Option<i32>,
        repo_id: Option<&str>,
        offset: u64,
        limit: u64,
    ) -> Result<Vec<activity::Model>, AppError>;
    async fn count_by_repo_ids_filtered(
        &self,
        repo_ids: Vec<String>,
        user_id: Option<i32>,
        repo_id: Option<&str>,
    ) -> Result<u64, AppError>;
}

pub struct DbActivityRepository {
    db: Arc<DatabaseConnection>,
}

impl DbActivityRepository {
    pub fn new(db: Arc<DatabaseConnection>) -> Self {
        Self { db }
    }
}

#[async_trait]
impl ActivityRepository for DbActivityRepository {
    async fn find_by_repo_ids_filtered(
        &self,
        repo_ids: Vec<String>,
        user_id: Option<i32>,
        repo_id: Option<&str>,
        offset: u64,
        limit: u64,
    ) -> Result<Vec<activity::Model>, AppError> {
        let mut query = activity::Entity::find()
            .filter(activity::Column::RepoId.is_in(repo_ids))
            .order_by_desc(activity::Column::CreatedAt);

        if let Some(uid) = user_id {
            query = query.filter(activity::Column::UserId.eq(uid));
        }
        if let Some(rid) = repo_id {
            query = query.filter(activity::Column::RepoId.eq(rid));
        }

        Ok(query.offset(offset).limit(limit).all(self.db.as_ref()).await?)
    }

    async fn count_by_repo_ids_filtered(
        &self,
        repo_ids: Vec<String>,
        user_id: Option<i32>,
        repo_id: Option<&str>,
    ) -> Result<u64, AppError> {
        let mut query = activity::Entity::find()
            .filter(activity::Column::RepoId.is_in(repo_ids));

        if let Some(uid) = user_id {
            query = query.filter(activity::Column::UserId.eq(uid));
        }
        if let Some(rid) = repo_id {
            query = query.filter(activity::Column::RepoId.eq(rid));
        }

        Ok(query.count(self.db.as_ref()).await?)
    }
}
