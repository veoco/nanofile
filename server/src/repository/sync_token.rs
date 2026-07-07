use async_trait::async_trait;
use sea_orm::{ActiveModelTrait, ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter, Set};
use std::sync::Arc;

use crate::entity::sync_token;
use crate::error::AppError;

#[async_trait]
pub trait SyncTokenRepository: Send + Sync {
    async fn find_by_repo_and_user(
        &self,
        repo_id: &str,
        user_id: i32,
    ) -> Result<Option<sync_token::Model>, AppError>;
    async fn find_by_token(&self, token: &str) -> Result<Option<sync_token::Model>, AppError>;
    async fn create(
        &self,
        repo_id: &str,
        user_id: i32,
        token: String,
        client_peername: Option<String>,
        now: i64,
    ) -> Result<(), AppError>;
    async fn delete_by_repo(&self, repo_id: &str) -> Result<(), AppError>;
    async fn delete_by_user_and_peer(&self, user_id: i32, peer_id: &str) -> Result<u64, AppError>;
}

pub struct DbSyncTokenRepository {
    db: Arc<DatabaseConnection>,
}

impl DbSyncTokenRepository {
    pub fn new(db: Arc<DatabaseConnection>) -> Self {
        Self { db }
    }
}

#[async_trait]
impl SyncTokenRepository for DbSyncTokenRepository {
    async fn find_by_repo_and_user(
        &self,
        repo_id: &str,
        user_id: i32,
    ) -> Result<Option<sync_token::Model>, AppError> {
        Ok(sync_token::Entity::find()
            .filter(sync_token::Column::RepoId.eq(repo_id))
            .filter(sync_token::Column::UserId.eq(user_id))
            .one(self.db.as_ref())
            .await?)
    }

    async fn find_by_token(&self, token: &str) -> Result<Option<sync_token::Model>, AppError> {
        Ok(sync_token::Entity::find()
            .filter(sync_token::Column::Token.eq(token))
            .one(self.db.as_ref())
            .await?)
    }

    async fn create(
        &self,
        repo_id: &str,
        user_id: i32,
        token: String,
        peer_name: Option<String>,
        now: i64,
    ) -> Result<(), AppError> {
        sync_token::ActiveModel {
            id: sea_orm::NotSet,
            repo_id: Set(repo_id.to_string()),
            user_id: Set(user_id),
            token: Set(token),
            peer_name: Set(peer_name),
            created_at: Set(now),
            expires_at: Set(None),
            peer_id: Set(None),
            peer_ip: Set(None),
            client_version: Set(None),
            last_sync_time: Set(None),
        }
        .insert(self.db.as_ref())
        .await?;
        Ok(())
    }

    async fn delete_by_repo(&self, repo_id: &str) -> Result<(), AppError> {
        sync_token::Entity::delete_many()
            .filter(sync_token::Column::RepoId.eq(repo_id))
            .exec(self.db.as_ref())
            .await?;
        Ok(())
    }

    async fn delete_by_user_and_peer(&self, user_id: i32, peer_id: &str) -> Result<u64, AppError> {
        let result = sync_token::Entity::delete_many()
            .filter(sync_token::Column::UserId.eq(user_id))
            .filter(sync_token::Column::PeerId.eq(peer_id))
            .exec(self.db.as_ref())
            .await?;
        Ok(result.rows_affected)
    }
}
