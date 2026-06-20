use sea_orm::{ActiveModelTrait, ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter};
use sha2::{Digest, Sha256};

use crate::entity::user_2fa_backup_code;

pub struct BackupCodeManager;

impl BackupCodeManager {
    pub fn generate_codes(count: usize) -> Vec<String> {
        (0..count)
            .map(|_| crate::auth::token::generate_backup_code())
            .collect()
    }

    pub fn hash_code(code: &str) -> String {
        let mut hasher = Sha256::new();
        hasher.update(code.as_bytes());
        hex::encode(hasher.finalize())
    }

    pub async fn store_codes(
        db: &DatabaseConnection,
        user_id: i32,
        codes: &[String],
    ) -> Result<(), sea_orm::DbErr> {
        let now = chrono::Utc::now().timestamp();
        for code in codes {
            let model = user_2fa_backup_code::ActiveModel {
                id: sea_orm::NotSet,
                user_id: sea_orm::Set(user_id),
                code_hash: sea_orm::Set(Self::hash_code(code)),
                used: sea_orm::Set(false),
                used_at: sea_orm::NotSet,
                created_at: sea_orm::Set(now),
            };
            user_2fa_backup_code::Entity::insert(model).exec(db).await?;
        }
        Ok(())
    }

    pub async fn delete_all_for_user(
        db: &DatabaseConnection,
        user_id: i32,
    ) -> Result<(), sea_orm::DbErr> {
        user_2fa_backup_code::Entity::delete_many()
            .filter(user_2fa_backup_code::Column::UserId.eq(user_id))
            .exec(db)
            .await?;
        Ok(())
    }

    pub async fn verify_code(
        db: &DatabaseConnection,
        user_id: i32,
        code: &str,
    ) -> Result<bool, sea_orm::DbErr> {
        let hash = Self::hash_code(code);
        let record = user_2fa_backup_code::Entity::find()
            .filter(user_2fa_backup_code::Column::UserId.eq(user_id))
            .filter(user_2fa_backup_code::Column::CodeHash.eq(&hash))
            .filter(user_2fa_backup_code::Column::Used.eq(false))
            .one(db)
            .await?;

        match record {
            Some(model) => {
                let now = chrono::Utc::now().timestamp();
                let mut active: user_2fa_backup_code::ActiveModel = model.into();
                active.used = sea_orm::Set(true);
                active.used_at = sea_orm::Set(Some(now));
                active.update(db).await?;
                Ok(true)
            }
            None => Ok(false),
        }
    }
}
