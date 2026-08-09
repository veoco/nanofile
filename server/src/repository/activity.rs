use async_trait::async_trait;
use sea_orm::{
    ColumnTrait, DatabaseConnection, EntityTrait, PaginatorTrait, QueryFilter, QueryOrder,
    QuerySelect,
};
use std::sync::Arc;

use base::error::AppError;
use infra::entity::activity;

/// SQLite has a ~999 bound on bound parameters; chunk IN / NOT IN lists.
const IN_BATCH: usize = 500;

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

/// Apply the `op_user` and single-repo filters to a query.
fn apply_extra_filters(
    mut query: sea_orm::Select<activity::Entity>,
    user_id: Option<i32>,
    repo_id: Option<&str>,
) -> sea_orm::Select<activity::Entity> {
    if let Some(uid) = user_id {
        query = query.filter(activity::Column::UserId.eq(uid));
    }
    if let Some(rid) = repo_id {
        query = query.filter(activity::Column::RepoId.eq(rid));
    }
    query
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
        // The original access control was `(repo_id IN accessible) OR
        // (user_id = requesting user)`. SQLite's OR optimization can't merge
        // two different indexes (repo_id vs user_id), so it fell back to a
        // full table scan. Split it into two index-friendly, disjoint branches:
        //   A: repo_id IN (accessible)         → idx_activities_repo_created
        //   B: user_id = requester AND repo_id NOT IN (accessible)
        //                                        → idx_activities_user_created
        // Branch B's NOT IN makes A and B disjoint (each row has one repo_id),
        // so results can be merged without dedup.
        if repo_ids.is_empty() && direct_user_id.is_none() {
            // No access condition — match everything (current behavior).
            let query = apply_extra_filters(activity::Entity::find(), user_id, repo_id);
            return Ok(query
                .order_by_desc(activity::Column::CreatedAt)
                .offset(offset)
                .limit(limit)
                .all(self.db.as_ref())
                .await?);
        }

        // Each branch fetches the top (offset+limit) by created_at; merging the
        // branches by created_at and slicing the page is correct because any
        // row in the global top (offset+limit) is within its own branch's top
        // (offset+limit).
        let need = offset.saturating_add(limit);
        let mut merged: Vec<activity::Model> = Vec::new();

        if !repo_ids.is_empty() {
            for chunk in repo_ids.chunks(IN_BATCH) {
                let query = apply_extra_filters(
                    activity::Entity::find().filter(activity::Column::RepoId.is_in(chunk)),
                    user_id,
                    repo_id,
                );
                merged.extend(
                    query
                        .order_by_desc(activity::Column::CreatedAt)
                        .limit(need)
                        .all(self.db.as_ref())
                        .await?,
                );
            }
        }

        if let Some(uid) = direct_user_id {
            let mut query = activity::Entity::find().filter(activity::Column::UserId.eq(uid));
            for chunk in repo_ids.chunks(IN_BATCH) {
                query = query.filter(activity::Column::RepoId.is_not_in(chunk));
            }
            let query = apply_extra_filters(query, user_id, repo_id);
            merged.extend(
                query
                    .order_by_desc(activity::Column::CreatedAt)
                    .limit(need)
                    .all(self.db.as_ref())
                    .await?,
            );
        }

        // Merge-sort by created_at desc, then slice the requested page.
        merged.sort_by_key(|m| std::cmp::Reverse(m.created_at));
        Ok(merged
            .into_iter()
            .skip(offset as usize)
            .take(limit as usize)
            .collect())
    }

    async fn count_by_repo_ids_filtered(
        &self,
        repo_ids: Vec<String>,
        user_id: Option<i32>,
        repo_id: Option<&str>,
        direct_user_id: Option<i32>,
    ) -> Result<u64, AppError> {
        if repo_ids.is_empty() && direct_user_id.is_none() {
            let query = apply_extra_filters(activity::Entity::find(), user_id, repo_id);
            return Ok(query.count(self.db.as_ref()).await?);
        }

        // Disjoint branches (repo_id IN vs user_id = requester AND repo_id NOT
        // IN), so the total is the sum of the two indexed counts.
        let mut total = 0u64;
        if !repo_ids.is_empty() {
            for chunk in repo_ids.chunks(IN_BATCH) {
                let query = apply_extra_filters(
                    activity::Entity::find().filter(activity::Column::RepoId.is_in(chunk)),
                    user_id,
                    repo_id,
                );
                total += query.count(self.db.as_ref()).await?;
            }
        }
        if let Some(uid) = direct_user_id {
            let mut query = activity::Entity::find().filter(activity::Column::UserId.eq(uid));
            for chunk in repo_ids.chunks(IN_BATCH) {
                query = query.filter(activity::Column::RepoId.is_not_in(chunk));
            }
            let query = apply_extra_filters(query, user_id, repo_id);
            total += query.count(self.db.as_ref()).await?;
        }
        Ok(total)
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
