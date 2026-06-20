use async_trait::async_trait;
use sea_orm::{
    ActiveModelTrait, ColumnTrait, DatabaseConnection, DeleteResult, EntityTrait, QueryFilter, Set,
};
use std::sync::Arc;

use crate::entity::repo_member;
use crate::error::AppError;

#[async_trait]
pub trait MemberRepository: Send + Sync {
    async fn find_by_user_id(&self, user_id: i32) -> Result<Vec<repo_member::Model>, AppError>;
    async fn find_by_repo_id(&self, repo_id: &str) -> Result<Vec<repo_member::Model>, AppError>;
    async fn find_by_repo_and_user(
        &self,
        repo_id: &str,
        user_id: i32,
    ) -> Result<Option<repo_member::Model>, AppError>;
    async fn create(&self, model: repo_member::ActiveModel)
    -> Result<repo_member::Model, AppError>;
    async fn update_permission(
        &self,
        repo_id: &str,
        user_id: i32,
        permission: &str,
    ) -> Result<(), AppError>;
    async fn delete_by_repo_and_user(
        &self,
        repo_id: &str,
        user_id: i32,
    ) -> Result<DeleteResult, AppError>;
    async fn delete_by_repo(&self, repo_id: &str) -> Result<DeleteResult, AppError>;
}

pub struct DbMemberRepository {
    db: Arc<DatabaseConnection>,
}

impl DbMemberRepository {
    pub fn new(db: Arc<DatabaseConnection>) -> Self {
        Self { db }
    }
}

#[async_trait]
impl MemberRepository for DbMemberRepository {
    async fn find_by_user_id(&self, user_id: i32) -> Result<Vec<repo_member::Model>, AppError> {
        Ok(repo_member::Entity::find()
            .filter(repo_member::Column::UserId.eq(user_id))
            .all(self.db.as_ref())
            .await?)
    }

    async fn find_by_repo_id(&self, repo_id: &str) -> Result<Vec<repo_member::Model>, AppError> {
        Ok(repo_member::Entity::find()
            .filter(repo_member::Column::RepoId.eq(repo_id))
            .all(self.db.as_ref())
            .await?)
    }

    async fn find_by_repo_and_user(
        &self,
        repo_id: &str,
        user_id: i32,
    ) -> Result<Option<repo_member::Model>, AppError> {
        Ok(repo_member::Entity::find()
            .filter(repo_member::Column::RepoId.eq(repo_id))
            .filter(repo_member::Column::UserId.eq(user_id))
            .one(self.db.as_ref())
            .await?)
    }

    async fn create(
        &self,
        model: repo_member::ActiveModel,
    ) -> Result<repo_member::Model, AppError> {
        let result = model.insert(self.db.as_ref()).await?;
        Ok(result)
    }

    async fn update_permission(
        &self,
        repo_id: &str,
        user_id: i32,
        permission: &str,
    ) -> Result<(), AppError> {
        let member = self.find_by_repo_and_user(repo_id, user_id).await?;
        if let Some(m) = member {
            let mut active: repo_member::ActiveModel = m.into();
            active.permission = Set(permission.to_string());
            active.update(self.db.as_ref()).await?;
        }
        Ok(())
    }

    async fn delete_by_repo_and_user(
        &self,
        repo_id: &str,
        user_id: i32,
    ) -> Result<DeleteResult, AppError> {
        let result = repo_member::Entity::delete_many()
            .filter(repo_member::Column::RepoId.eq(repo_id))
            .filter(repo_member::Column::UserId.eq(user_id))
            .exec(self.db.as_ref())
            .await?;
        Ok(result)
    }

    async fn delete_by_repo(&self, repo_id: &str) -> Result<DeleteResult, AppError> {
        let result = repo_member::Entity::delete_many()
            .filter(repo_member::Column::RepoId.eq(repo_id))
            .exec(self.db.as_ref())
            .await?;
        Ok(result)
    }
}
