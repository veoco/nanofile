use async_trait::async_trait;
use sea_orm::{
    ActiveModelTrait, ColumnTrait, DatabaseConnection, DeleteResult, EntityTrait, QueryFilter, Set,
};
use std::sync::Arc;

use crate::entity::upload_link;
use crate::error::AppError;

#[async_trait]
pub trait UploadLinkRepository: Send + Sync {
    async fn find_by_repo_and_path(
        &self,
        repo_id: &str,
        path: &str,
    ) -> Result<Vec<upload_link::Model>, AppError>;
    async fn find_by_creator_id(
        &self,
        creator_id: i32,
    ) -> Result<Vec<upload_link::Model>, AppError>;
    async fn find_by_token(&self, token: &str) -> Result<Option<upload_link::Model>, AppError>;
    async fn find_by_repo_id(&self, repo_id: &str) -> Result<Vec<upload_link::Model>, AppError>;
    async fn find_expired(&self) -> Result<Vec<upload_link::Model>, AppError>;
    async fn insert(&self, model: upload_link::ActiveModel)
    -> Result<upload_link::Model, AppError>;
    async fn delete_by_token_and_user(
        &self,
        token: &str,
        user_id: i32,
    ) -> Result<DeleteResult, AppError>;
    async fn delete_by_id_and_user(&self, id: i32, user_id: i32) -> Result<DeleteResult, AppError>;
    async fn update(
        &self,
        token: &str,
        user_id: i32,
        expire_at: Option<Option<i64>>,
        password: Option<Option<String>>,
        description: Option<Option<String>>,
    ) -> Result<bool, AppError>;
}

pub struct DbUploadLinkRepository {
    db: Arc<DatabaseConnection>,
}

impl DbUploadLinkRepository {
    pub fn new(db: Arc<DatabaseConnection>) -> Self {
        Self { db }
    }
}

#[async_trait]
impl UploadLinkRepository for DbUploadLinkRepository {
    async fn find_by_repo_and_path(
        &self,
        repo_id: &str,
        path: &str,
    ) -> Result<Vec<upload_link::Model>, AppError> {
        Ok(upload_link::Entity::find()
            .filter(upload_link::Column::RepoId.eq(repo_id))
            .filter(upload_link::Column::Path.eq(path))
            .all(self.db.as_ref())
            .await?)
    }

    async fn find_by_creator_id(
        &self,
        creator_id: i32,
    ) -> Result<Vec<upload_link::Model>, AppError> {
        Ok(upload_link::Entity::find()
            .filter(upload_link::Column::CreatorId.eq(creator_id))
            .all(self.db.as_ref())
            .await?)
    }

    async fn find_by_token(&self, token: &str) -> Result<Option<upload_link::Model>, AppError> {
        Ok(upload_link::Entity::find()
            .filter(upload_link::Column::Token.eq(token))
            .one(self.db.as_ref())
            .await?)
    }

    async fn find_by_repo_id(&self, repo_id: &str) -> Result<Vec<upload_link::Model>, AppError> {
        Ok(upload_link::Entity::find()
            .filter(upload_link::Column::RepoId.eq(repo_id))
            .all(self.db.as_ref())
            .await?)
    }

    async fn find_expired(&self) -> Result<Vec<upload_link::Model>, AppError> {
        let now = chrono::Utc::now().timestamp();
        Ok(upload_link::Entity::find()
            .filter(upload_link::Column::ExpiresAt.is_not_null())
            .filter(upload_link::Column::ExpiresAt.lte(now))
            .all(self.db.as_ref())
            .await?)
    }

    async fn insert(
        &self,
        model: upload_link::ActiveModel,
    ) -> Result<upload_link::Model, AppError> {
        Ok(model.insert(self.db.as_ref()).await?)
    }

    async fn delete_by_token_and_user(
        &self,
        token: &str,
        user_id: i32,
    ) -> Result<DeleteResult, AppError> {
        Ok(upload_link::Entity::delete_many()
            .filter(upload_link::Column::Token.eq(token))
            .filter(upload_link::Column::CreatorId.eq(user_id))
            .exec(self.db.as_ref())
            .await?)
    }

    async fn delete_by_id_and_user(&self, id: i32, user_id: i32) -> Result<DeleteResult, AppError> {
        Ok(upload_link::Entity::delete_many()
            .filter(upload_link::Column::Id.eq(id))
            .filter(upload_link::Column::CreatorId.eq(user_id))
            .exec(self.db.as_ref())
            .await?)
    }

    async fn update(
        &self,
        token: &str,
        user_id: i32,
        expire_at: Option<Option<i64>>,
        password: Option<Option<String>>,
        description: Option<Option<String>>,
    ) -> Result<bool, AppError> {
        let link = upload_link::Entity::find()
            .filter(upload_link::Column::Token.eq(token))
            .filter(upload_link::Column::CreatorId.eq(user_id))
            .one(self.db.as_ref())
            .await?
            .ok_or_else(|| AppError::NotFound("Upload link not found".into()))?;

        let mut active: upload_link::ActiveModel = link.into();

        if let Some(val) = expire_at {
            if let Some(ts) = val {
                active.expires_at = Set(Some(ts));
            } else {
                active.expires_at = Set(None);
            }
        }
        if let Some(val) = password {
            active.password = Set(val);
        }
        if let Some(val) = description {
            active.description = Set(val);
        }

        active.update(self.db.as_ref()).await?;
        Ok(true)
    }
}
