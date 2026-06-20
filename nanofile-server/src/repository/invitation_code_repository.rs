use async_trait::async_trait;
use sea_orm::{
    ActiveModelTrait, ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter, QueryOrder, Set,
};
use std::sync::Arc;

use crate::entity::invitation_code;
use crate::error::AppError;

#[async_trait]
pub trait InvitationCodeRepository: Send + Sync {
    async fn find_by_creator_id(
        &self,
        creator_id: i32,
    ) -> Result<Vec<invitation_code::Model>, AppError>;
    async fn create(
        &self,
        code: String,
        email: Option<String>,
        creator_id: i32,
        now: i64,
    ) -> Result<(), AppError>;
    async fn delete_by_id_and_creator(
        &self,
        id: i32,
        creator_id: i32,
    ) -> Result<(), AppError>;
}

pub struct DbInvitationCodeRepository {
    db: Arc<DatabaseConnection>,
}

impl DbInvitationCodeRepository {
    pub fn new(db: Arc<DatabaseConnection>) -> Self {
        Self { db }
    }
}

#[async_trait]
impl InvitationCodeRepository for DbInvitationCodeRepository {
    async fn find_by_creator_id(
        &self,
        creator_id: i32,
    ) -> Result<Vec<invitation_code::Model>, AppError> {
        Ok(invitation_code::Entity::find()
            .filter(invitation_code::Column::CreatorId.eq(creator_id))
            .order_by_desc(invitation_code::Column::CreatedAt)
            .all(self.db.as_ref())
            .await?)
    }

    async fn create(
        &self,
        code: String,
        email: Option<String>,
        creator_id: i32,
        now: i64,
    ) -> Result<(), AppError> {
        invitation_code::ActiveModel {
            id: sea_orm::NotSet,
            code: Set(code),
            email: Set(email),
            creator_id: Set(creator_id),
            created_at: Set(now),
            used_by: Set(None),
            used_at: Set(None),
        }
        .insert(self.db.as_ref())
        .await?;
        Ok(())
    }

    async fn delete_by_id_and_creator(
        &self,
        id: i32,
        creator_id: i32,
    ) -> Result<(), AppError> {
        let result = invitation_code::Entity::delete_many()
            .filter(invitation_code::Column::Id.eq(id))
            .filter(invitation_code::Column::CreatorId.eq(creator_id))
            .exec(self.db.as_ref())
            .await?;

        if result.rows_affected == 0 {
            return Err(AppError::NotFound("Invitation code not found.".to_string()));
        }
        Ok(())
    }
}
