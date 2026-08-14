use async_trait::async_trait;
use sea_orm::{
    ActiveModelTrait, ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter, Set, TryIntoModel,
};
use std::sync::Arc;

use base::error::AppError;
use infra::entity::wiki2_publish;

#[async_trait]
pub trait Wiki2PublishRepository: Send + Sync {
    /// Find the publish config for a repo (None when not published).
    async fn find_by_repo_id(
        &self,
        repo_id: &str,
    ) -> Result<Option<wiki2_publish::Model>, AppError>;
    /// Check whether a custom publish URL is already taken by another repo.
    async fn find_by_publish_url(
        &self,
        publish_url: &str,
    ) -> Result<Option<wiki2_publish::Model>, AppError>;
    /// Batch-load publish configs for many repos in one query.
    async fn find_by_repo_ids(
        &self,
        repo_ids: &[String],
    ) -> Result<Vec<wiki2_publish::Model>, AppError>;
    /// Insert or update the publish config for a repo.
    async fn upsert(
        &self,
        repo_id: &str,
        publish_url: &str,
        username: &str,
        enable_server_render: bool,
    ) -> Result<wiki2_publish::Model, AppError>;
    /// Remove the publish config (unpublish).
    async fn delete_by_repo_id(&self, repo_id: &str) -> Result<(), AppError>;
}

pub struct DbWiki2PublishRepository {
    db: Arc<DatabaseConnection>,
}

impl DbWiki2PublishRepository {
    pub fn new(db: Arc<DatabaseConnection>) -> Self {
        Self { db }
    }
}

#[async_trait]
impl Wiki2PublishRepository for DbWiki2PublishRepository {
    async fn find_by_repo_id(
        &self,
        repo_id: &str,
    ) -> Result<Option<wiki2_publish::Model>, AppError> {
        Ok(wiki2_publish::Entity::find()
            .filter(wiki2_publish::Column::RepoId.eq(repo_id))
            .one(self.db.as_ref())
            .await?)
    }

    async fn find_by_publish_url(
        &self,
        publish_url: &str,
    ) -> Result<Option<wiki2_publish::Model>, AppError> {
        Ok(wiki2_publish::Entity::find()
            .filter(wiki2_publish::Column::PublishUrl.eq(publish_url))
            .one(self.db.as_ref())
            .await?)
    }

    async fn find_by_repo_ids(
        &self,
        repo_ids: &[String],
    ) -> Result<Vec<wiki2_publish::Model>, AppError> {
        if repo_ids.is_empty() {
            return Ok(Vec::new());
        }
        Ok(wiki2_publish::Entity::find()
            .filter(wiki2_publish::Column::RepoId.is_in(repo_ids))
            .all(self.db.as_ref())
            .await?)
    }

    async fn upsert(
        &self,
        repo_id: &str,
        publish_url: &str,
        username: &str,
        enable_server_render: bool,
    ) -> Result<wiki2_publish::Model, AppError> {
        if let Some(existing) = self.find_by_repo_id(repo_id).await? {
            let mut model: wiki2_publish::ActiveModel = existing.into();
            model.publish_url = Set(publish_url.to_string());
            model.enable_server_render = Set(enable_server_render);
            return Ok(model.update(self.db.as_ref()).await?);
        }
        let model = wiki2_publish::ActiveModel {
            repo_id: Set(repo_id.to_string()),
            publish_url: Set(publish_url.to_string()),
            username: Set(username.to_string()),
            created_at: Set(chrono::Utc::now().timestamp()),
            visit_count: Set(0),
            enable_server_render: Set(enable_server_render),
        };
        // `repo_id` is a string primary key (not auto-increment), so plain
        // `insert()` would try to reload by the numeric last_insert_id and
        // fail. Exec the insert and reconstruct the model from the ActiveModel.
        wiki2_publish::Entity::insert(model.clone())
            .exec(self.db.as_ref())
            .await?;
        Ok(model
            .try_into_model()
            .map_err(|_| AppError::internal("failed to build publish model"))?)
    }

    async fn delete_by_repo_id(&self, repo_id: &str) -> Result<(), AppError> {
        wiki2_publish::Entity::delete_many()
            .filter(wiki2_publish::Column::RepoId.eq(repo_id))
            .exec(self.db.as_ref())
            .await?;
        Ok(())
    }
}
