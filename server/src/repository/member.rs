use async_trait::async_trait;
use sea_orm::{
    ActiveModelTrait, ColumnTrait, ConnectionTrait, DatabaseBackend, DatabaseConnection,
    DeleteResult, EntityTrait, QueryFilter, Set, Statement,
};
use std::sync::Arc;

use base::error::AppError;
use infra::entity::repo_member;

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

    /// Look up a repo's owner and the user's membership permission in a single
    /// LEFT JOIN query. Returns `(owner_id, permission)` where `permission` is
    /// `None` if the user is not a member.
    ///
    /// This is the canonical permission check query used by `domain::permission`.
    async fn find_repo_owner_and_permission(
        &self,
        repo_id: &str,
        user_id: i32,
    ) -> Result<Option<(i32, Option<String>)>, AppError>;
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
        repo_member::Entity::update_many()
            .filter(repo_member::Column::RepoId.eq(repo_id))
            .filter(repo_member::Column::UserId.eq(user_id))
            .set(repo_member::ActiveModel {
                permission: Set(permission.to_string()),
                ..Default::default()
            })
            .exec(self.db.as_ref())
            .await?;
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

    async fn find_repo_owner_and_permission(
        &self,
        repo_id: &str,
        user_id: i32,
    ) -> Result<Option<(i32, Option<String>)>, AppError> {
        let row = self
            .db
            .query_one(Statement::from_sql_and_values(
                DatabaseBackend::Sqlite,
                "SELECT r.owner_id, m.permission FROM repos r \
                 LEFT JOIN repo_members m ON r.id = m.repo_id AND m.user_id = $1 \
                 WHERE r.id = $2",
                vec![user_id.into(), repo_id.to_owned().into()],
            ))
            .await?
            .map(|r| {
                let owner_id: i32 = r.try_get("", "owner_id").unwrap_or(0);
                let permission: Option<String> = r.try_get("", "permission").ok();
                (owner_id, permission)
            });
        Ok(row)
    }
}
