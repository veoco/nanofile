use async_trait::async_trait;
use sea_orm::{
    ActiveModelTrait, ColumnTrait, DatabaseConnection, DeleteResult, EntityTrait, QueryFilter, Set,
};
use std::sync::Arc;

use crate::entity::share_link;
use crate::error::AppError;

#[async_trait]
pub trait ShareLinkRepository: Send + Sync {
    async fn find_by_repo_and_path(
        &self,
        repo_id: &str,
        path: &str,
    ) -> Result<Vec<share_link::Model>, AppError>;
    async fn find_by_creator_id(&self, creator_id: i32)
    -> Result<Vec<share_link::Model>, AppError>;
    async fn find_by_token(&self, token: &str) -> Result<Option<share_link::Model>, AppError>;
    async fn find_by_id(&self, id: i32) -> Result<Option<share_link::Model>, AppError>;
    async fn find_all(&self) -> Result<Vec<share_link::Model>, AppError>;
    async fn insert(&self, model: share_link::ActiveModel) -> Result<share_link::Model, AppError>;
    async fn delete_by_token_and_user(
        &self,
        token: &str,
        user_id: i32,
    ) -> Result<DeleteResult, AppError>;
    async fn delete_by_token(&self, token: &str) -> Result<DeleteResult, AppError>;
    async fn update_expire_and_description(
        &self,
        token: &str,
        user_id: i32,
        expire_at: Option<Option<i64>>,
        description: Option<Option<String>>,
    ) -> Result<bool, AppError>;
    /// Increment the view count for a share link by its ID.
    async fn increment_view_cnt(&self, id: i32) -> Result<(), AppError>;
    /// Delete expired share links (where expires_at < now).
    async fn delete_expired(&self, now: i64) -> Result<u64, AppError>;
}

pub struct DbShareLinkRepository {
    db: Arc<DatabaseConnection>,
}

impl DbShareLinkRepository {
    pub fn new(db: Arc<DatabaseConnection>) -> Self {
        Self { db }
    }
}

#[async_trait]
impl ShareLinkRepository for DbShareLinkRepository {
    async fn find_by_repo_and_path(
        &self,
        repo_id: &str,
        path: &str,
    ) -> Result<Vec<share_link::Model>, AppError> {
        Ok(share_link::Entity::find()
            .filter(share_link::Column::RepoId.eq(repo_id))
            .filter(share_link::Column::Path.eq(path))
            .all(self.db.as_ref())
            .await?)
    }

    async fn find_by_creator_id(
        &self,
        creator_id: i32,
    ) -> Result<Vec<share_link::Model>, AppError> {
        Ok(share_link::Entity::find()
            .filter(share_link::Column::CreatorId.eq(creator_id))
            .all(self.db.as_ref())
            .await?)
    }

    async fn find_by_token(&self, token: &str) -> Result<Option<share_link::Model>, AppError> {
        Ok(share_link::Entity::find()
            .filter(share_link::Column::Token.eq(token))
            .one(self.db.as_ref())
            .await?)
    }

    async fn find_by_id(&self, id: i32) -> Result<Option<share_link::Model>, AppError> {
        Ok(share_link::Entity::find_by_id(id)
            .one(self.db.as_ref())
            .await?)
    }

    async fn find_all(&self) -> Result<Vec<share_link::Model>, AppError> {
        Ok(share_link::Entity::find().all(self.db.as_ref()).await?)
    }

    async fn insert(&self, model: share_link::ActiveModel) -> Result<share_link::Model, AppError> {
        Ok(model.insert(self.db.as_ref()).await?)
    }

    async fn delete_by_token_and_user(
        &self,
        token: &str,
        user_id: i32,
    ) -> Result<DeleteResult, AppError> {
        Ok(share_link::Entity::delete_many()
            .filter(share_link::Column::Token.eq(token))
            .filter(share_link::Column::CreatorId.eq(user_id))
            .exec(self.db.as_ref())
            .await?)
    }

    async fn delete_by_token(&self, token: &str) -> Result<DeleteResult, AppError> {
        Ok(share_link::Entity::delete_many()
            .filter(share_link::Column::Token.eq(token))
            .exec(self.db.as_ref())
            .await?)
    }

    async fn update_expire_and_description(
        &self,
        token: &str,
        user_id: i32,
        expire_at: Option<Option<i64>>,
        description: Option<Option<String>>,
    ) -> Result<bool, AppError> {
        let link = share_link::Entity::find()
            .filter(share_link::Column::Token.eq(token))
            .filter(share_link::Column::CreatorId.eq(user_id))
            .one(self.db.as_ref())
            .await?
            .ok_or_else(|| AppError::NotFound("Share link not found".into()))?;

        let mut active: share_link::ActiveModel = link.into();

        if let Some(val) = expire_at {
            active.expires_at = Set(val);
        }
        if let Some(val) = description {
            active.description = Set(val);
        }

        active.update(self.db.as_ref()).await?;
        Ok(true)
    }

    async fn increment_view_cnt(&self, id: i32) -> Result<(), AppError> {
        if let Some(link) = share_link::Entity::find_by_id(id)
            .one(self.db.as_ref())
            .await?
        {
            let mut active: share_link::ActiveModel = link.into();
            active.view_cnt = Set(active.view_cnt.unwrap() + 1);
            active.update(self.db.as_ref()).await?;
        }
        Ok(())
    }

    async fn delete_expired(&self, now: i64) -> Result<u64, AppError> {
        let result = share_link::Entity::delete_many()
            .filter(share_link::Column::ExpiresAt.is_not_null())
            .filter(share_link::Column::ExpiresAt.lt(now))
            .exec(self.db.as_ref())
            .await?;
        Ok(result.rows_affected)
    }
}
