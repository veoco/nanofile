use async_trait::async_trait;
use sea_orm::{ActiveModelTrait, ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter, Set};
use std::sync::Arc;

use crate::entity::user_2fa_backup_code;
use crate::error::AppError;

#[async_trait]
pub trait User2faBackupCodeRepository: Send + Sync {
    async fn find_by_user(
        &self,
        user_id: i32,
    ) -> Result<Vec<user_2fa_backup_code::Model>, AppError>;
    async fn find_by_code_hash(
        &self,
        code_hash: &str,
    ) -> Result<Option<user_2fa_backup_code::Model>, AppError>;
    async fn delete_by_user(&self, user_id: i32) -> Result<(), AppError>;
    async fn insert(&self, model: user_2fa_backup_code::ActiveModel) -> Result<(), AppError>;
    async fn mark_as_used(&self, code_hash: &str, used_at: i64) -> Result<(), AppError>;
}

pub struct DbUser2faBackupCodeRepository {
    db: Arc<DatabaseConnection>,
}

impl DbUser2faBackupCodeRepository {
    pub fn new(db: Arc<DatabaseConnection>) -> Self {
        Self { db }
    }
}

#[async_trait]
impl User2faBackupCodeRepository for DbUser2faBackupCodeRepository {
    async fn find_by_user(
        &self,
        user_id: i32,
    ) -> Result<Vec<user_2fa_backup_code::Model>, AppError> {
        Ok(user_2fa_backup_code::Entity::find()
            .filter(user_2fa_backup_code::Column::UserId.eq(user_id))
            .all(self.db.as_ref())
            .await?)
    }

    async fn find_by_code_hash(
        &self,
        code_hash: &str,
    ) -> Result<Option<user_2fa_backup_code::Model>, AppError> {
        Ok(user_2fa_backup_code::Entity::find()
            .filter(user_2fa_backup_code::Column::CodeHash.eq(code_hash))
            .one(self.db.as_ref())
            .await?)
    }

    async fn delete_by_user(&self, user_id: i32) -> Result<(), AppError> {
        user_2fa_backup_code::Entity::delete_many()
            .filter(user_2fa_backup_code::Column::UserId.eq(user_id))
            .exec(self.db.as_ref())
            .await?;
        Ok(())
    }

    async fn insert(&self, model: user_2fa_backup_code::ActiveModel) -> Result<(), AppError> {
        model.insert(self.db.as_ref()).await?;
        Ok(())
    }

    async fn mark_as_used(&self, code_hash: &str, used_at: i64) -> Result<(), AppError> {
        let code = user_2fa_backup_code::Entity::find()
            .filter(user_2fa_backup_code::Column::CodeHash.eq(code_hash))
            .one(self.db.as_ref())
            .await?
            .ok_or_else(|| crate::error::AppError::NotFound("backup code not found".into()))?;
        let mut active: user_2fa_backup_code::ActiveModel = code.into();
        active.used = Set(true);
        active.used_at = Set(Some(used_at));
        active.update(self.db.as_ref()).await?;
        Ok(())
    }
}
