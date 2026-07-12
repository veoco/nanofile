use async_trait::async_trait;
use sea_orm::{ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter};
use std::sync::Arc;

use base::error::AppError;
use infra::entity::group_member;

#[async_trait]
pub trait GroupMemberRepository: Send + Sync {
    async fn find_by_user_id(&self, user_id: i32) -> Result<Vec<group_member::Model>, AppError>;
    async fn find_by_group_id(&self, group_id: i32) -> Result<Vec<group_member::Model>, AppError>;
}

pub struct DbGroupMemberRepository {
    db: Arc<DatabaseConnection>,
}

impl DbGroupMemberRepository {
    pub fn new(db: Arc<DatabaseConnection>) -> Self {
        Self { db }
    }
}

#[async_trait]
impl GroupMemberRepository for DbGroupMemberRepository {
    async fn find_by_user_id(&self, user_id: i32) -> Result<Vec<group_member::Model>, AppError> {
        Ok(group_member::Entity::find()
            .filter(group_member::Column::UserId.eq(user_id))
            .all(self.db.as_ref())
            .await?)
    }

    async fn find_by_group_id(&self, group_id: i32) -> Result<Vec<group_member::Model>, AppError> {
        Ok(group_member::Entity::find()
            .filter(group_member::Column::GroupId.eq(group_id))
            .all(self.db.as_ref())
            .await?)
    }
}
