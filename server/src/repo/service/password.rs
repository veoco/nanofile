use sea_orm::{ActiveModelTrait, DatabaseConnection, Set};

use crate::error::AppError;
use crate::repository::Repositories;

/// Service for encrypted repo password operations.
pub struct PasswordService;

impl PasswordService {
    /// Set the password for an encrypted repo.
    ///
    /// Verifies the password against the stored magic, derives the file
    /// encryption key, and caches it.
    pub async fn set_password(
        password_manager: &crate::crypto::password_manager::PasswordManager,
        repos: &Repositories,
        repo_id: &str,
        user_id: i32,
        password: &str,
    ) -> Result<(), AppError> {
        // Load the repo
        let repo_model = repos
            .repo
            .find_by_id(repo_id)
            .await?
            .ok_or_else(|| AppError::NotFound("repo not found".into()))?;

        // Check that it's encrypted
        if repo_model.encrypted == 0 {
            return Err(AppError::BadRequest("repo is not encrypted".into()));
        }

        let magic = repo_model
            .magic
            .as_deref()
            .ok_or_else(|| AppError::BadRequest("repo has no magic".into()))?;

        let random_key = repo_model
            .random_key
            .as_deref()
            .ok_or_else(|| AppError::BadRequest("repo has no random_key".into()))?;

        let salt = if repo_model.enc_version >= 3 {
            repo_model.salt.as_str()
        } else {
            ""
        };

        // Set the password (verify + derive + cache)
        password_manager
            .set_password(
                repo_id,
                user_id,
                password,
                repo_model.enc_version as i32,
                magic,
                random_key,
                salt,
            )
            .await
    }

    /// Change the password for an encrypted repo.
    ///
    /// 1. Verify old password
    /// 2. Generate new magic from new password + repo_id
    /// 3. Decrypt random_key with old password -> get secret_key
    /// 4. Re-encrypt secret_key with new password -> new random_key
    /// 5. Update repo's magic and random_key in DB
    pub async fn change_password(
        password_manager: &crate::crypto::password_manager::PasswordManager,
        repos: &Repositories,
        db: &DatabaseConnection,
        repo_id: &str,
        user_id: i32,
        old_password: &str,
        new_password: &str,
    ) -> Result<(), AppError> {
        // Load the repo
        let repo_model = repos
            .repo
            .find_by_id(repo_id)
            .await?
            .ok_or_else(|| AppError::NotFound("repo not found".into()))?;

        if repo_model.encrypted == 0 {
            return Err(AppError::BadRequest("repo is not encrypted".into()));
        }

        let enc_version = repo_model.enc_version as i32;
        let magic = repo_model
            .magic
            .as_deref()
            .ok_or_else(|| AppError::BadRequest("repo has no magic".into()))?;
        let random_key = repo_model
            .random_key
            .as_deref()
            .ok_or_else(|| AppError::BadRequest("repo has no random_key".into()))?;
        let salt = if enc_version >= 3 {
            repo_model.salt.as_str()
        } else {
            ""
        };

        // 1. Verify old password via password_manager
        password_manager
            .set_password(
                repo_id,
                user_id,
                old_password,
                enc_version,
                magic,
                random_key,
                salt,
            )
            .await?;

        // 2. Generate new magic
        let new_magic =
            crate::crypto::key_derivation::generate_magic(repo_id, new_password, enc_version, salt)
                .map_err(|e| AppError::BadRequest(format!("magic generation failed: {e}")))?;

        // 3. Decrypt the old random_key to get the secret key (the actual file key).
        use aes::cipher::{BlockModeDecrypt, BlockModeEncrypt, KeyIvInit, block_padding::Pkcs7};
        let old_derived =
            crate::crypto::key_derivation::derive_key(old_password, enc_version, salt)
                .map_err(|e| AppError::BadRequest(format!("key derivation failed: {e}")))?;
        let random_key_bytes = hex::decode(random_key)
            .map_err(|_| AppError::BadRequest("invalid random_key hex".into()))?;
        let old_cipher =
            cbc::Decryptor::<aes::Aes256>::new_from_slices(&old_derived.0, &old_derived.1)
                .map_err(|e| AppError::BadRequest(format!("cipher init: {e}")))?;
        let secret_key = old_cipher
            .decrypt_padded_vec::<Pkcs7>(&random_key_bytes)
            .map_err(|e| AppError::BadRequest(format!("failed to decrypt random_key: {e}")))?;

        // 4. Re-encrypt with new password (same format: raw AES-CBC with PKCS7 padding).
        let new_derived =
            crate::crypto::key_derivation::derive_key(new_password, enc_version, salt)
                .map_err(|e| AppError::BadRequest(format!("key derivation failed: {e}")))?;
        let new_cipher =
            cbc::Encryptor::<aes::Aes256>::new_from_slices(&new_derived.0, &new_derived.1)
                .map_err(|e| AppError::BadRequest(format!("cipher init: {e}")))?;
        let new_random_key = new_cipher.encrypt_padded_vec::<Pkcs7>(&secret_key);
        let new_random_key_hex = hex::encode(&new_random_key);

        // 5. Update repo in DB
        let mut active: crate::entity::repo::ActiveModel = repo_model.into();
        active.magic = Set(Some(new_magic));
        active.random_key = Set(Some(new_random_key_hex));
        active.updated_at = Set(chrono::Utc::now().timestamp());
        active.update(db).await?;

        // Remove cached old password
        password_manager.remove_password(repo_id, user_id).await;

        Ok(())
    }
}
