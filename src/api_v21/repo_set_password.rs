use axum::{
    Json,
    extract::{Path, Query, State},
};
use sea_orm::{ActiveModelTrait, EntityTrait, Set};
use serde::Deserialize;
use std::collections::HashMap;
use std::sync::Arc;

use crate::AppState;
use crate::auth::middleware::AuthUser;
use crate::entity::repo;
use crate::error::AppError;

/// Request body for setting a repo password.
#[derive(Deserialize)]
pub struct SetPasswordRequest {
    pub password: Option<String>,
}

/// Request body for changing a repo password.
#[derive(Deserialize)]
pub struct ChangePasswordRequest {
    pub old_password: Option<String>,
    pub new_password: Option<String>,
}

/// POST /api/v2.1/repos/{repo_id}/set-password/
///
/// Set the password for an encrypted repo. The server verifies the password
/// against the stored magic, derives the file encryption key, and caches it.
///
/// This matches seahub's RepoSetPassword.post().
pub async fn set_password_v21(
    auth: AuthUser,
    State(state): State<Arc<AppState>>,
    Path(repo_id): Path<String>,
    Json(body): Json<SetPasswordRequest>,
) -> Result<Json<serde_json::Value>, AppError> {
    let password = body
        .password
        .ok_or_else(|| AppError::BadRequest("password required".into()))?;

    set_repo_password_inner(&state, &repo_id, auth.user_id, &password).await?;

    Ok(Json(serde_json::json!({"success": true})))
}

/// Shared inner logic for setting a repo password.
///
/// Used by both the v2.1 endpoint and the v2 endpoint.
pub(crate) async fn set_repo_password_inner(
    state: &AppState,
    repo_id: &str,
    user_id: i32,
    password: &str,
) -> Result<(), AppError> {
    // Load the repo
    let repo_model = repo::Entity::find_by_id(repo_id)
        .one(state.db.as_ref())
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
    state
        .password_manager
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

/// PUT /api/v2.1/repos/{repo_id}/set-password/?operation=change-password
///
/// Change an encrypted repo's password.
///
/// This re-encrypts the random_key (48-byte encrypted secret key) with the
/// new password and updates the stored magic. No file blocks are touched.
///
/// Matches seahub's RepoSetPassword.put() with operation=change-password.
pub async fn change_password_v21(
    auth: AuthUser,
    State(state): State<Arc<AppState>>,
    Path(repo_id): Path<String>,
    Query(params): Query<HashMap<String, String>>,
    Json(body): Json<ChangePasswordRequest>,
) -> Result<Json<serde_json::Value>, AppError> {
    let operation = params.get("operation").map(|s| s.as_str());

    match operation {
        Some("change-password") => {
            let old_password = body
                .old_password
                .ok_or_else(|| AppError::BadRequest("old_password required".into()))?;
            let new_password = body
                .new_password
                .ok_or_else(|| AppError::BadRequest("new_password required".into()))?;

            change_repo_password_inner(&state, &repo_id, auth.user_id, &old_password, &new_password)
                .await
                .map(|_| Json(serde_json::json!({"success": true})))
        }
        Some("check-password") => {
            // Check if password is set for this repo
            let is_set = state
                .password_manager
                .is_password_set(&repo_id, auth.user_id)
                .await;
            Ok(Json(serde_json::json!({"is_set": is_set})))
        }
        _ => Err(AppError::BadRequest(
            "unknown operation; use change-password or check-password".into(),
        )),
    }
}

/// Change the password for an encrypted repo.
///
/// 1. Verify old password
/// 2. Generate new magic from new password + repo_id
/// 3. Decrypt random_key with old password → get secret_key
/// 4. Re-encrypt secret_key with new password → new random_key
/// 5. Update repo's magic and random_key in DB
async fn change_repo_password_inner(
    state: &AppState,
    repo_id: &str,
    user_id: i32,
    old_password: &str,
    new_password: &str,
) -> Result<(), AppError> {
    // Load the repo
    let repo_model = repo::Entity::find_by_id(repo_id)
        .one(state.db.as_ref())
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
    state
        .password_manager
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
    // The random_key stored in the DB is raw AES-256-CBC ciphertext (48 bytes,
    // no IV prefix), encrypted with a wrapping key/iv derived from the password.
    // Use PKCS7 padding to decrypt and strip the 16-byte padding from the 32-byte
    // secret key.
    use aes::cipher::{BlockModeDecrypt, BlockModeEncrypt, KeyIvInit, block_padding::Pkcs7};
    let old_derived = crate::crypto::key_derivation::derive_key(old_password, enc_version, salt)
        .map_err(|e| AppError::BadRequest(format!("key derivation failed: {e}")))?;
    let random_key_bytes = hex::decode(random_key)
        .map_err(|_| AppError::BadRequest("invalid random_key hex".into()))?;
    let old_cipher = cbc::Decryptor::<aes::Aes256>::new_from_slices(&old_derived.0, &old_derived.1)
        .map_err(|e| AppError::BadRequest(format!("cipher init: {e}")))?;
    let secret_key = old_cipher
        .decrypt_padded_vec::<Pkcs7>(&random_key_bytes)
        .map_err(|e| AppError::BadRequest(format!("failed to decrypt random_key: {e}")))?;

    // 4. Re-encrypt with new password (same format: raw AES-CBC with PKCS7 padding).
    let new_derived = crate::crypto::key_derivation::derive_key(new_password, enc_version, salt)
        .map_err(|e| AppError::BadRequest(format!("key derivation failed: {e}")))?;
    let new_cipher = cbc::Encryptor::<aes::Aes256>::new_from_slices(&new_derived.0, &new_derived.1)
        .map_err(|e| AppError::BadRequest(format!("cipher init: {e}")))?;
    let new_random_key = new_cipher.encrypt_padded_vec::<Pkcs7>(&secret_key);
    let new_random_key_hex = hex::encode(&new_random_key);

    // 5. Update repo in DB
    let mut active: repo::ActiveModel = repo_model.into();
    active.magic = Set(Some(new_magic));
    active.random_key = Set(Some(new_random_key_hex));
    active.updated_at = Set(chrono::Utc::now().timestamp());
    active.update(state.db.as_ref()).await?;

    // Remove cached old password
    state
        .password_manager
        .remove_password(repo_id, user_id)
        .await;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_set_password_request_deserialize() {
        let json = r#"{"password": "test123"}"#;
        let req: SetPasswordRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.password, Some("test123".to_string()));
    }

    #[test]
    fn test_set_password_request_missing_password() {
        let json = r#"{}"#;
        let req: SetPasswordRequest = serde_json::from_str(json).unwrap();
        assert!(req.password.is_none());
    }

    #[test]
    fn test_change_password_request_deserialize() {
        let json = r#"{"old_password": "old", "new_password": "new"}"#;
        let req: ChangePasswordRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.old_password, Some("old".to_string()));
        assert_eq!(req.new_password, Some("new".to_string()));
    }
}
