use async_trait::async_trait;
use sea_orm::{ColumnTrait, DatabaseConnection, EntityTrait, PaginatorTrait, QueryFilter};
use std::sync::Arc;

use base::error::AppError;
use infra::entity::group_member;

#[async_trait]
pub trait GroupMemberRepository: Send + Sync {
    async fn find_by_user_id(&self, user_id: i32) -> Result<Vec<group_member::Model>, AppError>;
    async fn find_by_group_id(&self, group_id: i32) -> Result<Vec<group_member::Model>, AppError>;
    /// Fetch members for several groups in one chunked `IN` query.
    async fn find_by_group_ids(
        &self,
        group_ids: &[i32],
    ) -> Result<Vec<group_member::Model>, AppError>;
    /// Count members of a group without loading their rows.
    async fn count_by_group_id(&self, group_id: i32) -> Result<i64, AppError>;
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

    async fn find_by_group_ids(
        &self,
        group_ids: &[i32],
    ) -> Result<Vec<group_member::Model>, AppError> {
        if group_ids.is_empty() {
            return Ok(Vec::new());
        }
        const IN_BATCH: usize = 500;
        let mut out = Vec::new();
        for chunk in group_ids.chunks(IN_BATCH) {
            out.extend(
                group_member::Entity::find()
                    .filter(group_member::Column::GroupId.is_in(chunk.to_vec()))
                    .all(self.db.as_ref())
                    .await?,
            );
        }
        Ok(out)
    }

    async fn count_by_group_id(&self, group_id: i32) -> Result<i64, AppError> {
        let n = group_member::Entity::find()
            .filter(group_member::Column::GroupId.eq(group_id))
            .count(self.db.as_ref())
            .await?;
        Ok(n as i64)
    }
}
