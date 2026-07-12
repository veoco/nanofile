use async_trait::async_trait;
use sea_orm::{
    ActiveModelTrait, ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter, QueryOrder, Set,
};
use std::sync::Arc;

use base::error::AppError;
use infra::entity::commit;

pub struct CreateCommitParams {
    pub repo_id: String,
    pub commit_id: String,
    pub root_id: String,
    pub parent_id: Option<String>,
    pub second_parent_id: Option<String>,
    pub creator_name: String,
    pub description: String,
    pub ctime: i64,
    pub version: i8,
}

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
    async fn insert_commit(&self, params: CreateCommitParams) -> Result<commit::Model, AppError>;
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

    async fn insert_commit(&self, params: CreateCommitParams) -> Result<commit::Model, AppError> {
        let model = commit::ActiveModel {
            id: sea_orm::NotSet,
            repo_id: Set(params.repo_id),
            commit_id: Set(params.commit_id),
            root_id: Set(params.root_id),
            parent_id: Set(params.parent_id),
            second_parent_id: Set(params.second_parent_id),
            creator_name: Set(params.creator_name),
            description: Set(params.description),
            ctime: Set(params.ctime),
            version: Set(params.version),
        };
        Ok(model.insert(self.db.as_ref()).await?)
    }

    async fn find_all_ordered_by_ctime_desc(&self) -> Result<Vec<commit::Model>, AppError> {
        Ok(commit::Entity::find()
            .order_by_desc(commit::Column::Ctime)
            .all(self.db.as_ref())
            .await?)
    }
}
