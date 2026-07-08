use async_trait::async_trait;
use sea_orm::{
    ActiveModelTrait, ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter, QueryOrder,
};
use std::sync::Arc;

use crate::entity::commit;
use crate::error::AppError;

#[async_trait]
pub trait CommitRepository: Send + Sync {
    async fn find_by_id(&self, commit_id: &str) -> Result<Option<commit::Model>, AppError>;
    async fn find_by_repo_and_commit_id(
        &self,
        repo_id: &str,
        commit_id: &str,
    ) -> Result<Option<commit::Model>, AppError>;
    async fn find_by_repo_id(&self, repo_id: &str) -> Result<Vec<commit::Model>, AppError>;
    async fn insert(&self, model: commit::ActiveModel) -> Result<commit::Model, AppError>;
    async fn find_all_ordered_by_ctime_desc(&self) -> Result<Vec<commit::Model>, AppError>;
}

pub struct DbCommitRepository {
    db: Arc<DatabaseConnection>,
}

impl DbCommitRepository {
    pub fn new(db: Arc<DatabaseConnection>) -> Self {
        Self { db }
    }
}

#[async_trait]
impl CommitRepository for DbCommitRepository {
    async fn find_by_id(&self, commit_id: &str) -> Result<Option<commit::Model>, AppError> {
        Ok(commit::Entity::find()
            .filter(commit::Column::CommitId.eq(commit_id))
            .one(self.db.as_ref())
            .await?)
    }

    async fn find_by_repo_and_commit_id(
        &self,
        repo_id: &str,
        commit_id: &str,
    ) -> Result<Option<commit::Model>, AppError> {
        Ok(commit::Entity::find()
            .filter(commit::Column::RepoId.eq(repo_id))
            .filter(commit::Column::CommitId.eq(commit_id))
            .one(self.db.as_ref())
            .await?)
    }

    async fn find_by_repo_id(&self, repo_id: &str) -> Result<Vec<commit::Model>, AppError> {
        Ok(commit::Entity::find()
            .filter(commit::Column::RepoId.eq(repo_id))
            .all(self.db.as_ref())
            .await?)
    }

    async fn insert(&self, model: commit::ActiveModel) -> Result<commit::Model, AppError> {
        Ok(model.insert(self.db.as_ref()).await?)
    }

    async fn find_all_ordered_by_ctime_desc(&self) -> Result<Vec<commit::Model>, AppError> {
        Ok(commit::Entity::find()
            .order_by_desc(commit::Column::Ctime)
            .all(self.db.as_ref())
            .await?)
    }
}
