use async_trait::async_trait;
use sea_orm::{
    ActiveModelTrait, ColumnTrait, DatabaseConnection, DeleteResult, EntityTrait, QueryFilter, Set,
};
use std::sync::Arc;

use base::error::AppError;
use infra::entity::upload_link;

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
    async fn find_all(&self) -> Result<Vec<upload_link::Model>, AppError>;
    async fn find_by_repo_id(&self, repo_id: &str) -> Result<Vec<upload_link::Model>, AppError>;
    async fn find_expired(&self) -> Result<Vec<upload_link::Model>, AppError>;
    async fn insert(&self, model: upload_link::ActiveModel)
    -> Result<upload_link::Model, AppError>;
    async fn delete_by_token_and_user(
        &self,
        token: &str,
        user_id: i32,
    ) -> Result<DeleteResult, AppError>;
    async fn delete_by_token(&self, token: &str) -> Result<DeleteResult, AppError>;
    async fn delete_by_id_and_user(&self, id: i32, user_id: i32) -> Result<DeleteResult, AppError>;
    async fn update(
        &self,
        token: &str,
        user_id: i32,
        expire_at: Option<Option<i64>>,
        password: Option<Option<String>>,
        description: Option<Option<String>>,
    ) -> Result<bool, AppError>;
    /// Delete expired upload links (where expires_at < now).
    async fn delete_expired(&self, now: i64) -> Result<u64, AppError>;
    /// Increment the view count for an upload link by its ID.
    async fn increment_view_cnt(&self, id: i32) -> Result<(), AppError>;
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

    async fn find_all(&self) -> Result<Vec<upload_link::Model>, AppError> {
        Ok(upload_link::Entity::find().all(self.db.as_ref()).await?)
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

    async fn delete_by_token(&self, token: &str) -> Result<DeleteResult, AppError> {
        Ok(upload_link::Entity::delete_many()
            .filter(upload_link::Column::Token.eq(token))
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
        let mut active = upload_link::ActiveModel {
            ..Default::default()
        };

        if let Some(val) = expire_at {
            active.expires_at = Set(val);
        }
        if let Some(val) = password {
            active.password = Set(val);
        }
        if let Some(val) = description {
            active.description = Set(val);
        }

        let result = upload_link::Entity::update_many()
            .filter(upload_link::Column::Token.eq(token))
            .filter(upload_link::Column::CreatorId.eq(user_id))
            .set(active)
            .exec(self.db.as_ref())
            .await?;
        if result.rows_affected == 0 {
            return Err(AppError::NotFound("Upload link not found".into()));
        }
        Ok(true)
    }

    async fn increment_view_cnt(&self, id: i32) -> Result<(), AppError> {
        if let Some(link) = upload_link::Entity::find_by_id(id)
            .one(self.db.as_ref())
            .await?
        {
            upload_link::Entity::update_many()
                .filter(upload_link::Column::Id.eq(id))
                .set(upload_link::ActiveModel {
                    view_cnt: Set(link.view_cnt + 1),
                    ..Default::default()
                })
                .exec(self.db.as_ref())
                .await?;
        }
        Ok(())
    }

    async fn delete_expired(&self, now: i64) -> Result<u64, AppError> {
        let result = upload_link::Entity::delete_many()
            .filter(upload_link::Column::ExpiresAt.is_not_null())
            .filter(upload_link::Column::ExpiresAt.lt(now))
            .exec(self.db.as_ref())
            .await?;
        Ok(result.rows_affected)
    }
}
