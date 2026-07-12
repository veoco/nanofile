use async_trait::async_trait;
use sea_orm::{
    ActiveModelTrait, ColumnTrait, DatabaseConnection, DeleteResult, EntityTrait, QueryFilter, Set,
};
use std::sync::Arc;

use base::error::AppError;
use infra::entity::share_link;

/// Parameters for creating a share link.
pub struct CreateShareLinkParams {
    pub repo_id: String,
    pub creator_id: i32,
    pub path: String,
    pub token: String,
    pub password: Option<String>,
    pub expires_at: Option<i64>,
    pub created_at: i64,
    pub s_type: String,
    pub description: Option<String>,
}

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
    /// Create a share link from typed parameters.
    async fn create_share_link(
        &self,
        params: CreateShareLinkParams,
    ) -> Result<share_link::Model, AppError>;
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
    /// Update share link fields (password, expire_at, description).
    /// Each field uses `Option<Option<T>>` — None = no change, Some(None) = clear, Some(Some(v)) = set.
    async fn update_share_link_fields(
        &self,
        id: i32,
        password: Option<Option<String>>,
        expires_at: Option<Option<i64>>,
        description: Option<Option<String>>,
    ) -> Result<(), AppError>;
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
    async fn create_share_link(
        &self,
        params: CreateShareLinkParams,
    ) -> Result<share_link::Model, AppError> {
        let model = share_link::ActiveModel {
            id: sea_orm::NotSet,
            repo_id: Set(params.repo_id),
            creator_id: Set(params.creator_id),
            path: Set(params.path),
            token: Set(params.token),
            password: Set(params.password),
            expires_at: Set(params.expires_at),
            created_at: Set(params.created_at),
            s_type: Set(params.s_type),
            view_cnt: Set(0i64),
            description: Set(params.description),
        };
        Ok(model.insert(self.db.as_ref()).await?)
    }
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
        let mut active: share_link::ActiveModel = share_link::ActiveModel {
            ..Default::default()
        };
        if let Some(val) = expire_at {
            active.expires_at = Set(val);
        }
        if let Some(val) = description {
            active.description = Set(val);
        }

        share_link::Entity::update_many()
            .filter(share_link::Column::Token.eq(token))
            .filter(share_link::Column::CreatorId.eq(user_id))
            .set(active)
            .exec(self.db.as_ref())
            .await?;
        Ok(true)
    }

    async fn increment_view_cnt(&self, id: i32) -> Result<(), AppError> {
        if let Some(link) = share_link::Entity::find_by_id(id)
            .one(self.db.as_ref())
            .await?
        {
            let new_cnt = link.view_cnt + 1;
            share_link::Entity::update_many()
                .filter(share_link::Column::Id.eq(id))
                .set(share_link::ActiveModel {
                    view_cnt: Set(new_cnt),
                    ..Default::default()
                })
                .exec(self.db.as_ref())
                .await?;
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

    async fn update_share_link_fields(
        &self,
        id: i32,
        password: Option<Option<String>>,
        expires_at: Option<Option<i64>>,
        description: Option<Option<String>>,
    ) -> Result<(), AppError> {
        let mut active: share_link::ActiveModel = share_link::ActiveModel {
            ..Default::default()
        };
        if let Some(val) = password {
            active.password = Set(val);
        }
        if let Some(val) = expires_at {
            active.expires_at = Set(val);
        }
        if let Some(val) = description {
            active.description = Set(val);
        }
        share_link::Entity::update_many()
            .filter(share_link::Column::Id.eq(id))
            .set(active)
            .exec(self.db.as_ref())
            .await?;
        Ok(())
    }
}
