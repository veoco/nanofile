use async_trait::async_trait;
use sea_orm::{
    ActiveModelTrait, ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter, QueryOrder, Set,
};
use std::sync::Arc;

use base::error::AppError;
use infra::entity::invitation_code;

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
    async fn delete_by_id_and_creator(&self, id: i32, creator_id: i32) -> Result<(), AppError>;

    // ── Methods for UI layer refactoring ───────────────────────────────
    /// Find an invitation code by its code string.
    async fn find_by_code(&self, code: &str) -> Result<Option<invitation_code::Model>, AppError>;
    /// Mark an invitation code as used by a specific user.
    async fn mark_as_used(&self, id: i32, used_by: i32, used_at: i64) -> Result<(), AppError>;
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

    async fn delete_by_id_and_creator(&self, id: i32, creator_id: i32) -> Result<(), AppError> {
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

    async fn find_by_code(&self, code: &str) -> Result<Option<invitation_code::Model>, AppError> {
        Ok(invitation_code::Entity::find()
            .filter(invitation_code::Column::Code.eq(code))
            .one(self.db.as_ref())
            .await?)
    }

    async fn mark_as_used(&self, id: i32, used_by: i32, used_at: i64) -> Result<(), AppError> {
        let result = invitation_code::Entity::update_many()
            .filter(invitation_code::Column::Id.eq(id))
            .filter(invitation_code::Column::UsedBy.is_null())
            .set(invitation_code::ActiveModel {
                used_by: Set(Some(used_by)),
                used_at: Set(Some(used_at)),
                ..Default::default()
            })
            .exec(self.db.as_ref())
            .await?;
        if result.rows_affected == 0 {
            // Check if the record exists to differentiate "not found" from "already used"
            if invitation_code::Entity::find_by_id(id)
                .one(self.db.as_ref())
                .await?
                .is_some()
            {
                return Err(AppError::BadRequest(
                    "This invitation code has already been used.".to_string(),
                ));
            }
            return Err(AppError::NotFound("Invitation code not found.".to_string()));
        }
        Ok(())
    }
}
