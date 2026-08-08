use async_trait::async_trait;
use sea_orm::{ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter, QueryOrder, Set};
use std::sync::Arc;

use base::error::AppError;
use infra::entity::repo_tag;

/// A tag definition within a repo.
pub struct TagInput {
    pub name: String,
    pub color: String,
}

#[async_trait]
pub trait RepoTagRepository: Send + Sync {
    async fn find_by_repo_id(&self, repo_id: &str) -> Result<Vec<repo_tag::Model>, AppError>;
    async fn find_by_id(&self, id: i32) -> Result<Option<repo_tag::Model>, AppError>;
    async fn find_by_repo_and_name(
        &self,
        repo_id: &str,
        name: &str,
    ) -> Result<Option<repo_tag::Model>, AppError>;
    /// Create a tag, returning the created row.
    async fn create(
        &self,
        repo_id: &str,
        name: &str,
        color: &str,
    ) -> Result<repo_tag::Model, AppError>;
    /// Bulk create tags, skipping names that already exist in the repo.
    async fn create_many(
        &self,
        repo_id: &str,
        items: &[TagInput],
    ) -> Result<Vec<repo_tag::Model>, AppError>;
    async fn update(&self, id: i32, name: &str, color: &str) -> Result<(), AppError>;
    /// Delete a tag (file_tags referencing it are removed via FK cascade).
    async fn delete_by_id(&self, id: i32) -> Result<(), AppError>;
}

pub struct DbRepoTagRepository {
    db: Arc<DatabaseConnection>,
}

impl DbRepoTagRepository {
    pub fn new(db: Arc<DatabaseConnection>) -> Self {
        Self { db }
    }
}

#[async_trait]
impl RepoTagRepository for DbRepoTagRepository {
    async fn find_by_repo_id(&self, repo_id: &str) -> Result<Vec<repo_tag::Model>, AppError> {
        Ok(repo_tag::Entity::find()
            .filter(repo_tag::Column::RepoId.eq(repo_id))
            .order_by_asc(repo_tag::Column::CreatedAt)
            .order_by_asc(repo_tag::Column::Id)
            .all(self.db.as_ref())
            .await?)
    }

    async fn find_by_id(&self, id: i32) -> Result<Option<repo_tag::Model>, AppError> {
        Ok(repo_tag::Entity::find_by_id(id)
            .one(self.db.as_ref())
            .await?)
    }

    async fn find_by_repo_and_name(
        &self,
        repo_id: &str,
        name: &str,
    ) -> Result<Option<repo_tag::Model>, AppError> {
        Ok(repo_tag::Entity::find()
            .filter(repo_tag::Column::RepoId.eq(repo_id))
            .filter(repo_tag::Column::Name.eq(name))
            .one(self.db.as_ref())
            .await?)
    }

    async fn create(
        &self,
        repo_id: &str,
        name: &str,
        color: &str,
    ) -> Result<repo_tag::Model, AppError> {
        Ok(repo_tag::Entity::insert(repo_tag::ActiveModel {
            id: sea_orm::NotSet,
            repo_id: Set(repo_id.to_string()),
            name: Set(name.to_string()),
            color: Set(color.to_string()),
            created_at: Set(chrono::Utc::now().timestamp()),
        })
        .exec_with_returning(self.db.as_ref())
        .await?)
    }

    async fn create_many(
        &self,
        repo_id: &str,
        items: &[TagInput],
    ) -> Result<Vec<repo_tag::Model>, AppError> {
        let db = self.db.as_ref();
        let mut out: Vec<repo_tag::Model> = Vec::new();
        for item in items {
            let existing = self.find_by_repo_and_name(repo_id, &item.name).await?;
            if let Some(tag) = existing {
                out.push(tag);
                continue;
            }
            let tag = repo_tag::Entity::insert(repo_tag::ActiveModel {
                id: sea_orm::NotSet,
                repo_id: Set(repo_id.to_string()),
                name: Set(item.name.clone()),
                color: Set(item.color.clone()),
                created_at: Set(chrono::Utc::now().timestamp()),
            })
            .exec_with_returning(db)
            .await?;
            out.push(tag);
        }
        Ok(out)
    }

    async fn update(&self, id: i32, name: &str, color: &str) -> Result<(), AppError> {
        repo_tag::Entity::update_many()
            .filter(repo_tag::Column::Id.eq(id))
            .set(repo_tag::ActiveModel {
                name: Set(name.to_string()),
                color: Set(color.to_string()),
                ..Default::default()
            })
            .exec(self.db.as_ref())
            .await?;
        Ok(())
    }

    async fn delete_by_id(&self, id: i32) -> Result<(), AppError> {
        repo_tag::Entity::delete_many()
            .filter(repo_tag::Column::Id.eq(id))
            .exec(self.db.as_ref())
            .await?;
        Ok(())
    }
}
