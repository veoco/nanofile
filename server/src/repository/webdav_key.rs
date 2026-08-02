use async_trait::async_trait;
use sea_orm::{
    ActiveModelTrait, ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter, QueryOrder, Set,
};
use std::sync::Arc;

use base::error::AppError;
use infra::entity::webdav_key;

#[async_trait]
pub trait WebdavKeyRepository: Send + Sync {
    /// Create a new WebDAV key and return the persisted model.
    async fn create(
        &self,
        repo_id: &str,
        user_id: i32,
        name: &str,
        key_hash: &str,
    ) -> Result<webdav_key::Model, AppError>;
    /// List all keys a user holds for a repo.
    async fn find_by_repo_and_user(
        &self,
        repo_id: &str,
        user_id: i32,
    ) -> Result<Vec<webdav_key::Model>, AppError>;
    /// List all keys for a repo (owner/admin view).
    async fn find_by_repo(&self, repo_id: &str) -> Result<Vec<webdav_key::Model>, AppError>;
    /// Find a key by id and repo (used before deletion).
    async fn find_by_id_and_repo(
        &self,
        key_id: i32,
        repo_id: &str,
    ) -> Result<Option<webdav_key::Model>, AppError>;
    /// Find an active key by repo + user + key hash (auth lookup).
    async fn find_by_repo_user_hash(
        &self,
        repo_id: &str,
        user_id: i32,
        key_hash: &str,
    ) -> Result<Option<webdav_key::Model>, AppError>;
    async fn delete_by_id(&self, key_id: i32) -> Result<(), AppError>;
    async fn update_last_used_at(&self, key_id: i32, ts: i64) -> Result<(), AppError>;
}

pub struct DbWebdavKeyRepository {
    db: Arc<DatabaseConnection>,
}

impl DbWebdavKeyRepository {
    pub fn new(db: Arc<DatabaseConnection>) -> Self {
        Self { db }
    }
}

#[async_trait]
impl WebdavKeyRepository for DbWebdavKeyRepository {
    async fn create(
        &self,
        repo_id: &str,
        user_id: i32,
        name: &str,
        key_hash: &str,
    ) -> Result<webdav_key::Model, AppError> {
        let model = webdav_key::ActiveModel {
            id: sea_orm::NotSet,
            repo_id: Set(repo_id.to_string()),
            user_id: Set(user_id),
            name: Set(name.to_string()),
            key_hash: Set(key_hash.to_string()),
            created_at: Set(chrono::Utc::now().timestamp()),
            last_used_at: Set(None),
        };
        Ok(model.insert(self.db.as_ref()).await?)
    }

    async fn find_by_repo_and_user(
        &self,
        repo_id: &str,
        user_id: i32,
    ) -> Result<Vec<webdav_key::Model>, AppError> {
        Ok(webdav_key::Entity::find()
            .filter(webdav_key::Column::RepoId.eq(repo_id))
            .filter(webdav_key::Column::UserId.eq(user_id))
            .order_by_desc(webdav_key::Column::CreatedAt)
            .all(self.db.as_ref())
            .await?)
    }

    async fn find_by_repo(&self, repo_id: &str) -> Result<Vec<webdav_key::Model>, AppError> {
        Ok(webdav_key::Entity::find()
            .filter(webdav_key::Column::RepoId.eq(repo_id))
            .order_by_desc(webdav_key::Column::CreatedAt)
            .all(self.db.as_ref())
            .await?)
    }

    async fn find_by_id_and_repo(
        &self,
        key_id: i32,
        repo_id: &str,
    ) -> Result<Option<webdav_key::Model>, AppError> {
        Ok(webdav_key::Entity::find()
            .filter(webdav_key::Column::Id.eq(key_id))
            .filter(webdav_key::Column::RepoId.eq(repo_id))
            .one(self.db.as_ref())
            .await?)
    }

    async fn find_by_repo_user_hash(
        &self,
        repo_id: &str,
        user_id: i32,
        key_hash: &str,
    ) -> Result<Option<webdav_key::Model>, AppError> {
        Ok(webdav_key::Entity::find()
            .filter(webdav_key::Column::RepoId.eq(repo_id))
            .filter(webdav_key::Column::UserId.eq(user_id))
            .filter(webdav_key::Column::KeyHash.eq(key_hash))
            .one(self.db.as_ref())
            .await?)
    }

    async fn delete_by_id(&self, key_id: i32) -> Result<(), AppError> {
        webdav_key::Entity::delete_many()
            .filter(webdav_key::Column::Id.eq(key_id))
            .exec(self.db.as_ref())
            .await?;
        Ok(())
    }

    async fn update_last_used_at(&self, key_id: i32, ts: i64) -> Result<(), AppError> {
        webdav_key::Entity::update_many()
            .filter(webdav_key::Column::Id.eq(key_id))
            .set(webdav_key::ActiveModel {
                last_used_at: Set(Some(ts)),
                ..Default::default()
            })
            .exec(self.db.as_ref())
            .await?;
        Ok(())
    }
}
