use async_trait::async_trait;
use sea_orm::{ActiveModelTrait, ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter, Set};
use std::sync::Arc;

use base::error::AppError;
use infra::entity::user_2fa_backup_code;

pub struct CreateBackupCodeParams {
    pub user_id: i32,
    pub code_hash: String,
    pub created_at: i64,
}

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
    async fn create_backup_code(
        &self,
        params: CreateBackupCodeParams,
    ) -> Result<user_2fa_backup_code::Model, AppError>;
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

    async fn create_backup_code(
        &self,
        params: CreateBackupCodeParams,
    ) -> Result<user_2fa_backup_code::Model, AppError> {
        let model = user_2fa_backup_code::ActiveModel {
            id: sea_orm::NotSet,
            user_id: Set(params.user_id),
            code_hash: Set(params.code_hash),
            used: Set(false),
            used_at: Set(None),
            created_at: Set(params.created_at),
        };
        Ok(model.insert(self.db.as_ref()).await?)
    }

    async fn mark_as_used(&self, code_hash: &str, used_at: i64) -> Result<(), AppError> {
        let result = user_2fa_backup_code::Entity::update_many()
            .filter(user_2fa_backup_code::Column::CodeHash.eq(code_hash))
            .set(user_2fa_backup_code::ActiveModel {
                used: Set(true),
                used_at: Set(Some(used_at)),
                ..Default::default()
            })
            .exec(self.db.as_ref())
            .await?;
        if result.rows_affected == 0 {
            return Err(base::error::AppError::NotFound(
                "backup code not found".into(),
            ));
        }
        Ok(())
    }
}
