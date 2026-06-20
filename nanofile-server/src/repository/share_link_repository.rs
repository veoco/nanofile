use async_trait::async_trait;
use sea_orm::{ColumnTrait, DatabaseConnection, DeleteResult, EntityTrait, QueryFilter};
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
    async fn delete_by_token_and_user(
        &self,
        token: &str,
        user_id: i32,
    ) -> Result<DeleteResult, AppError>;
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
}
