use sha2::{Digest, Sha256};

use crate::repository::Repositories;
use base::error::AppError;
use infra::entity::user_2fa_backup_code;

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
        repos: &Repositories,
        user_id: i32,
        codes: &[String],
    ) -> Result<(), AppError> {
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
            repos.user_2fa_backup_code.insert(model).await?;
        }
        Ok(())
    }

    pub async fn delete_all_for_user(repos: &Repositories, user_id: i32) -> Result<(), AppError> {
        repos.user_2fa_backup_code.delete_by_user(user_id).await
    }

    pub async fn verify_code(
        repos: &Repositories,
        user_id: i32,
        code: &str,
    ) -> Result<bool, AppError> {
        let hash = Self::hash_code(code);
        let codes = repos.user_2fa_backup_code.find_by_user(user_id).await?;
        let record = codes.into_iter().find(|c| c.code_hash == hash && !c.used);

        match record {
            Some(model) => {
                let now = chrono::Utc::now().timestamp();
                repos
                    .user_2fa_backup_code
                    .mark_as_used(&model.code_hash, now)
                    .await?;
                Ok(true)
            }
            None => Ok(false),
        }
    }
}
