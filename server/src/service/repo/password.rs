use crate::repository::Repositories;
use crate::repository::repo::UpdateRepoKeysParams;
use base::error::AppError;

/// Service for encrypted repo password operations.
pub struct PasswordService;

impl PasswordService {
    /// Set the password for an encrypted repo.
    ///
    /// Verifies the password against the stored verifier — `pwd_hash` when the
    /// library has one, `magic` otherwise — derives the file encryption key, and
    /// caches it.
    pub async fn set_password(
        password_manager: &infra::crypto::password_manager::PasswordManager,
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
                &infra::crypto::password_manager::RepoKeyMaterial {
                    enc_version: repo_model.enc_version as i32,
                    magic: repo_model.magic.as_deref(),
                    random_key,
                    salt,
                    pwd_hash: repo_model.pwd_hash.as_deref(),
                    pwd_hash_algo: repo_model.pwd_hash_algo.as_deref(),
                    pwd_hash_params: repo_model.pwd_hash_params.as_deref(),
                },
            )
            .await
    }

    /// Change the password for an encrypted repo.
    ///
    /// 1. Verify old password
    /// 2. Generate the new verifier (`pwd_hash` or `magic`) from the new password
    /// 3. Decrypt random_key with old password -> get secret_key
    /// 4. Re-encrypt secret_key with new password -> new random_key
    /// 5. Update the repo's verifier and random_key in DB
    pub async fn change_password(
        password_manager: &infra::crypto::password_manager::PasswordManager,
        repos: &Repositories,
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
                &infra::crypto::password_manager::RepoKeyMaterial {
                    enc_version,
                    magic: repo_model.magic.as_deref(),
                    random_key,
                    salt,
                    pwd_hash: repo_model.pwd_hash.as_deref(),
                    pwd_hash_algo: repo_model.pwd_hash_algo.as_deref(),
                    pwd_hash_params: repo_model.pwd_hash_params.as_deref(),
                },
            )
            .await?;

        // 2. Regenerate the verifier. A `pwd_hash` library keeps using its
        //    algorithm and parameters — upstream's `pwd_hash_algo` is a property
        //    of the library, not of the password — and must stay without a
        //    `magic` (the commit omits `magic` whenever `pwd_hash` is set).
        let new_verifier = match repo_model.pwd_hash_algo.as_deref() {
            Some(algo) => {
                let hash = infra::crypto::pwd_hash::derive_pwd_hash(
                    repo_id,
                    new_password,
                    enc_version,
                    salt,
                    algo,
                    repo_model.pwd_hash_params.as_deref(),
                )
                .map_err(|e| AppError::BadRequest(format!("pwd_hash generation failed: {e}")))?;
                UpdateRepoKeysParams {
                    magic: None,
                    pwd_hash: Some(hash),
                    pwd_hash_algo: Some(algo.to_string()),
                    pwd_hash_params: repo_model.pwd_hash_params.clone(),
                    ..Default::default()
                }
            }
            None => {
                // A library with neither verifier is corrupt: refuse rather than
                // rotating it into a state nothing can unlock.
                if repo_model.magic.is_none() {
                    return Err(AppError::BadRequest("repo has no magic".into()));
                }
                let new_magic = infra::crypto::key_derivation::generate_magic(
                    repo_id,
                    new_password,
                    enc_version,
                    salt,
                )
                .map_err(|e| AppError::BadRequest(format!("magic generation failed: {e}")))?;
                UpdateRepoKeysParams {
                    magic: Some(new_magic),
                    ..Default::default()
                }
            }
        };

        // 3. Decrypt the old random_key to get the secret key (the actual file key).
        use aes::cipher::{BlockModeDecrypt, BlockModeEncrypt, KeyIvInit, block_padding::Pkcs7};
        let old_derived =
            infra::crypto::key_derivation::derive_key(old_password, enc_version, salt)
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
            infra::crypto::key_derivation::derive_key(new_password, enc_version, salt)
                .map_err(|e| AppError::BadRequest(format!("key derivation failed: {e}")))?;
        let new_cipher =
            cbc::Encryptor::<aes::Aes256>::new_from_slices(&new_derived.0, &new_derived.1)
                .map_err(|e| AppError::BadRequest(format!("cipher init: {e}")))?;
        let new_random_key = new_cipher.encrypt_padded_vec::<Pkcs7>(&secret_key);

        // 5. Update repo in DB
        repos
            .repo
            .update_repo_keys(
                repo_id,
                UpdateRepoKeysParams {
                    random_key: Some(hex::encode(&new_random_key)),
                    ..new_verifier
                },
            )
            .await?;

        // Drop every cached key for this library, not just the caller's:
        // rotation rewrites the repo-wide verifier/`random_key`, so any other
        // session still holding the old key must stop being able to decrypt.
        // (`user_id` is intentionally unused here now.)
        let _ = user_id;
        password_manager.remove_repo(repo_id).await;

        Ok(())
    }
}
