use async_trait::async_trait;
use sea_orm::{ActiveModelTrait, ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter, Set};
use std::sync::Arc;

use crate::entity::{commit, repo};
use crate::error::AppError;

#[async_trait]
pub trait RepoRepository: Send + Sync {
    async fn find_by_id(&self, repo_id: &str) -> Result<Option<repo::Model>, AppError>;
    async fn find_by_owner_id(&self, user_id: i32) -> Result<Vec<repo::Model>, AppError>;
    async fn create(&self, model: repo::ActiveModel) -> Result<repo::Model, AppError>;
    async fn update(&self, model: repo::ActiveModel) -> Result<repo::Model, AppError>;
    async fn update_head_commit(
        &self,
        repo_id: &str,
        head_commit_id: Option<String>,
    ) -> Result<(), AppError>;
    async fn delete_by_id(&self, repo_id: &str) -> Result<(), AppError>;
    /// Get the root fs_id from the repo's head commit.
    async fn get_head_root_id(&self, repo_id: &str) -> Result<Option<String>, AppError>;
    /// Add a delta to the repo's size (can be negative).
    async fn adjust_size(&self, repo_id: &str, delta: i64) -> Result<(), AppError>;
    /// Update repo encryption keys (magic + random_key). Used by password change.
    async fn update_repo_keys(
        &self,
        repo_id: &str,
        magic: Option<String>,
        random_key: Option<String>,
    ) -> Result<(), AppError>;
}

pub struct DbRepoRepository {
    db: Arc<DatabaseConnection>,
}

impl DbRepoRepository {
    pub fn new(db: Arc<DatabaseConnection>) -> Self {
        Self { db }
    }
}

#[async_trait]
impl RepoRepository for DbRepoRepository {
    async fn find_by_id(&self, repo_id: &str) -> Result<Option<repo::Model>, AppError> {
        Ok(repo::Entity::find_by_id(repo_id)
            .one(self.db.as_ref())
            .await?)
    }

    async fn find_by_owner_id(&self, user_id: i32) -> Result<Vec<repo::Model>, AppError> {
        Ok(repo::Entity::find()
            .filter(repo::Column::OwnerId.eq(user_id))
            .all(self.db.as_ref())
            .await?)
    }

    async fn create(&self, model: repo::ActiveModel) -> Result<repo::Model, AppError> {
        // Extract the repo_id before insert (ActiveModel will be consumed)
        let repo_id = match &model.id {
            sea_orm::Set(id) => id.clone(),
            _ => return Err(AppError::Internal("repo id is required".into())),
        };
        repo::Entity::insert(model).exec(self.db.as_ref()).await?;
        self.find_by_id(&repo_id)
            .await?
            .ok_or_else(|| AppError::Internal("Failed to find created repo".into()))
    }

    async fn update(&self, model: repo::ActiveModel) -> Result<repo::Model, AppError> {
        let result = model.update(self.db.as_ref()).await?;
        Ok(result)
    }

    async fn update_head_commit(
        &self,
        repo_id: &str,
        head_commit_id: Option<String>,
    ) -> Result<(), AppError> {
        repo::Entity::update_many()
            .filter(repo::Column::Id.eq(repo_id))
            .set(repo::ActiveModel {
                head_commit_id: Set(head_commit_id),
                ..Default::default()
            })
            .exec(self.db.as_ref())
            .await?;
        Ok(())
    }

    async fn delete_by_id(&self, repo_id: &str) -> Result<(), AppError> {
        repo::Entity::delete_by_id(repo_id)
            .exec(self.db.as_ref())
            .await?;
        Ok(())
    }

    async fn get_head_root_id(&self, repo_id: &str) -> Result<Option<String>, AppError> {
        let repo = self.find_by_id(repo_id).await?;
        match repo {
            Some(r) => match r.head_commit_id {
                Some(head_id) => {
                    let head = commit::Entity::find()
                        .filter(commit::Column::RepoId.eq(repo_id))
                        .filter(commit::Column::CommitId.eq(&head_id))
                        .one(self.db.as_ref())
                        .await?;
                    Ok(head.map(|h| h.root_id))
                }
                None => Ok(None),
            },
            None => Ok(None),
        }
    }

    async fn adjust_size(&self, repo_id: &str, delta: i64) -> Result<(), AppError> {
        let repo = self
            .find_by_id(repo_id)
            .await?
            .ok_or_else(|| AppError::NotFound("Repo not found".into()))?;
        let new_size = (repo.size + delta).max(0);
        repo::Entity::update_many()
            .filter(repo::Column::Id.eq(repo_id))
            .set(repo::ActiveModel {
                size: Set(new_size),
                ..Default::default()
            })
            .exec(self.db.as_ref())
            .await?;
        Ok(())
    }

    async fn update_repo_keys(
        &self,
        repo_id: &str,
        magic: Option<String>,
        random_key: Option<String>,
    ) -> Result<(), AppError> {
        let now = chrono::Utc::now().timestamp();
        repo::Entity::update_many()
            .filter(repo::Column::Id.eq(repo_id))
            .set(repo::ActiveModel {
                magic: Set(magic),
                random_key: Set(random_key),
                updated_at: Set(now),
                ..Default::default()
            })
            .exec(self.db.as_ref())
            .await?;
        Ok(())
    }
}
