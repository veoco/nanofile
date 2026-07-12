use async_trait::async_trait;
use sea_orm::{
    ColumnTrait, Condition, DatabaseConnection, EntityTrait, PaginatorTrait, QueryFilter,
    QueryOrder, QuerySelect,
};
use std::sync::Arc;

use base::error::AppError;
use infra::entity::activity;

#[async_trait]
pub trait ActivityRepository: Send + Sync {
    async fn find_by_repo_ids_filtered(
        &self,
        repo_ids: Vec<String>,
        user_id: Option<i32>,
        repo_id: Option<&str>,
        offset: u64,
        limit: u64,
        direct_user_id: Option<i32>,
    ) -> Result<Vec<activity::Model>, AppError>;
    async fn count_by_repo_ids_filtered(
        &self,
        repo_ids: Vec<String>,
        user_id: Option<i32>,
        repo_id: Option<&str>,
        direct_user_id: Option<i32>,
    ) -> Result<u64, AppError>;
    async fn find_recent_by_user(
        &self,
        user_id: i32,
        limit: u64,
    ) -> Result<Vec<activity::Model>, AppError>;
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
        direct_user_id: Option<i32>,
    ) -> Result<Vec<activity::Model>, AppError> {
        // Build the access-control condition:
        //   (repo_id IN accessible_repo_ids) OR (user_id = direct_user_id)
        // This matches the original seafevents behavior: activities the user
        // should see come from either repo membership (shared repos) or
        // direct authorship (the user's own activity, via UserActivity fan-out).
        let mut access = Condition::any();
        if !repo_ids.is_empty() {
            access = access.add(activity::Column::RepoId.is_in(repo_ids));
        }
        if let Some(uid) = direct_user_id {
            access = access.add(activity::Column::UserId.eq(uid));
        }

        let mut query = activity::Entity::find()
            .filter(access)
            .order_by_desc(activity::Column::CreatedAt);

        if let Some(uid) = user_id {
            query = query.filter(activity::Column::UserId.eq(uid));
        }
        if let Some(rid) = repo_id {
            query = query.filter(activity::Column::RepoId.eq(rid));
        }

        Ok(query
            .offset(offset)
            .limit(limit)
            .all(self.db.as_ref())
            .await?)
    }

    async fn count_by_repo_ids_filtered(
        &self,
        repo_ids: Vec<String>,
        user_id: Option<i32>,
        repo_id: Option<&str>,
        direct_user_id: Option<i32>,
    ) -> Result<u64, AppError> {
        let mut access = Condition::any();
        if !repo_ids.is_empty() {
            access = access.add(activity::Column::RepoId.is_in(repo_ids));
        }
        if let Some(uid) = direct_user_id {
            access = access.add(activity::Column::UserId.eq(uid));
        }

        let mut query = activity::Entity::find().filter(access);

        if let Some(uid) = user_id {
            query = query.filter(activity::Column::UserId.eq(uid));
        }
        if let Some(rid) = repo_id {
            query = query.filter(activity::Column::RepoId.eq(rid));
        }

        Ok(query.count(self.db.as_ref()).await?)
    }

    async fn find_recent_by_user(
        &self,
        user_id: i32,
        limit: u64,
    ) -> Result<Vec<activity::Model>, AppError> {
        Ok(activity::Entity::find()
            .filter(activity::Column::UserId.eq(user_id))
            .order_by_desc(activity::Column::CreatedAt)
            .limit(limit)
            .all(self.db.as_ref())
            .await?)
    }
}
