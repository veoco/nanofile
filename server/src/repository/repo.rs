use async_trait::async_trait;
use sea_orm::{ActiveModelTrait, ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter, Set};
use std::sync::Arc;

use base::error::AppError;
use infra::entity::{commit, repo};

/// Parameters for creating a new repo.
pub struct CreateRepoParams {
    pub id: String,
    pub name: String,
    pub description: String,
    pub owner_id: i32,
    pub encrypted: i8,
    pub enc_version: i8,
    pub magic: Option<String>,
    pub random_key: Option<String>,
    pub salt: String,
    pub permission: String,
    pub created_at: i64,
    pub updated_at: i64,
}

#[async_trait]
pub trait RepoRepository: Send + Sync {
    async fn find_by_id(&self, repo_id: &str) -> Result<Option<repo::Model>, AppError>;
    async fn find_by_owner_id(&self, user_id: i32) -> Result<Vec<repo::Model>, AppError>;
    async fn create(&self, model: repo::ActiveModel) -> Result<repo::Model, AppError>;
    /// Create a repo from typed parameters.
    async fn create_repo(&self, params: CreateRepoParams) -> Result<repo::Model, AppError>;
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
    /// Rename a repo (owner-only).
    async fn rename_repo(&self, repo_id: &str, name: &str, updated_at: i64)
    -> Result<(), AppError>;
    /// Update repo name and/or description (owner-only).
    async fn update_repo_details(
        &self,
        repo_id: &str,
        name: Option<&str>,
        description: Option<&str>,
        updated_at: i64,
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
    async fn create_repo(&self, params: CreateRepoParams) -> Result<repo::Model, AppError> {
        let model = repo::ActiveModel {
            id: Set(params.id.clone()),
            name: Set(params.name),
            description: Set(params.description),
            owner_id: Set(params.owner_id),
            encrypted: Set(params.encrypted),
            enc_version: Set(params.enc_version),
            magic: Set(params.magic),
            random_key: Set(params.random_key),
            salt: Set(params.salt),
            head_commit_id: sea_orm::NotSet,
            permission: Set(params.permission),
            repo_version: Set(1),
            size: Set(0),
            created_at: Set(params.created_at),
            updated_at: Set(params.updated_at),
        };
        repo::Entity::insert(model).exec(self.db.as_ref()).await?;
        self.find_by_id(&params.id)
            .await?
            .ok_or_else(|| AppError::Internal("Failed to find created repo".into()))
    }
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

    async fn rename_repo(
        &self,
        repo_id: &str,
        name: &str,
        updated_at: i64,
    ) -> Result<(), AppError> {
        repo::Entity::update_many()
            .filter(repo::Column::Id.eq(repo_id))
            .set(repo::ActiveModel {
                name: Set(name.to_string()),
                updated_at: Set(updated_at),
                ..Default::default()
            })
            .exec(self.db.as_ref())
            .await?;
        Ok(())
    }

    async fn update_repo_details(
        &self,
        repo_id: &str,
        name: Option<&str>,
        description: Option<&str>,
        updated_at: i64,
    ) -> Result<(), AppError> {
        let mut active: repo::ActiveModel = repo::ActiveModel {
            ..Default::default()
        };
        if let Some(n) = name {
            active.name = Set(n.to_string());
        }
        if let Some(d) = description {
            active.description = Set(d.to_string());
        }
        active.updated_at = Set(updated_at);

        repo::Entity::update_many()
            .filter(repo::Column::Id.eq(repo_id))
            .set(active)
            .exec(self.db.as_ref())
            .await?;
        Ok(())
    }
}
