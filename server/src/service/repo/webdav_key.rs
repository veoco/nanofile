use base::error::AppError;
use infra::entity::webdav_key;
use sha2::{Digest, Sha256};

use crate::repository::Repositories;
use crate::service::auth::token::generate_api_token;

/// Hash a WebDAV key. Keys are server-generated high-entropy random strings,
/// so a fast hash (SHA-256) is appropriate — unlike user passwords, there is
/// no brute-force risk that would justify a slow KDF. This keeps per-request
/// auth cheap (WebDAV clients send credentials on every request).
pub fn hash_webdav_key(key: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(key.as_bytes());
    hex::encode(hasher.finalize())
}

/// Return whether the user is the repo owner or a member (any permission).
///
/// `find_repo_owner_and_permission` returns `Some((owner_id, None))` for a
/// non-member (LEFT JOIN yields a row with NULL permission), so a bare
/// `Option::is_none` check is not sufficient.
async fn has_repo_access(
    repos: &Repositories,
    repo_id: &str,
    user_id: i32,
) -> Result<bool, AppError> {
    match repos
        .member
        .find_repo_owner_and_permission(repo_id, user_id)
        .await?
    {
        Some((owner_id, _)) if owner_id == user_id => Ok(true),
        Some((_, Some(_))) => Ok(true),
        _ => Ok(false),
    }
}

/// Return whether the user is the repo owner or a server admin.
async fn is_owner_or_admin(
    repos: &Repositories,
    repo_id: &str,
    user_id: i32,
) -> Result<bool, AppError> {
    if let Some(repo) = repos.repo.find_by_id(repo_id).await?
        && repo.owner_id == user_id
    {
        return Ok(true);
    }
    if let Some(user) = repos.user.find_by_id(user_id).await?
        && user.is_admin
    {
        return Ok(true);
    }
    Ok(false)
}

/// Services for WebDAV key management. Keys are immutable — the server
/// generates a random secret, returns the plaintext exactly once, and stores
/// only its SHA-256 hash. A key can only be deleted, never modified.
pub struct WebdavKeyService;

impl WebdavKeyService {
    /// Generate a new WebDAV key for the given user on the given repo.
    /// Returns `(persisted_model, plaintext_key)`. The plaintext is shown once.
    pub async fn generate_key(
        repos: &Repositories,
        repo_id: &str,
        user_id: i32,
        name: &str,
    ) -> Result<(webdav_key::Model, String), AppError> {
        let repo = repos
            .repo
            .find_by_id(repo_id)
            .await?
            .ok_or_else(|| AppError::NotFound("Repository not found".into()))?;

        if repo.encrypted != 0 {
            return Err(AppError::BadRequest(
                "WebDAV is not supported for encrypted libraries".into(),
            ));
        }

        // Any member (including read-only) may generate keys for their own use.
        if !has_repo_access(repos, repo_id, user_id).await? {
            return Err(AppError::Forbidden);
        }

        let key = generate_api_token();
        let key_hash = hash_webdav_key(&key);
        let name = if name.trim().is_empty() {
            "default".to_string()
        } else {
            name.trim().to_string()
        };

        let model = repos
            .webdav_key
            .create(repo_id, user_id, &name, &key_hash)
            .await?;
        Ok((model, key))
    }

    /// List WebDAV keys. Owners/admins see all keys in the repo; regular
    /// members only see their own.
    pub async fn list_keys(
        repos: &Repositories,
        repo_id: &str,
        user_id: i32,
    ) -> Result<Vec<webdav_key::Model>, AppError> {
        repos
            .repo
            .find_by_id(repo_id)
            .await?
            .ok_or_else(|| AppError::NotFound("Repository not found".into()))?;

        if !has_repo_access(repos, repo_id, user_id).await? {
            return Err(AppError::Forbidden);
        }

        if is_owner_or_admin(repos, repo_id, user_id).await? {
            repos.webdav_key.find_by_repo(repo_id).await
        } else {
            repos
                .webdav_key
                .find_by_repo_and_user(repo_id, user_id)
                .await
        }
    }

    /// Delete a single WebDAV key. Users may delete their own keys; owners and
    /// admins may delete any key in the repo.
    pub async fn delete_key(
        repos: &Repositories,
        repo_id: &str,
        user_id: i32,
        key_id: i32,
    ) -> Result<(), AppError> {
        let key = repos
            .webdav_key
            .find_by_id_and_repo(key_id, repo_id)
            .await?
            .ok_or_else(|| AppError::NotFound("WebDAV key not found".into()))?;

        if !is_owner_or_admin(repos, repo_id, user_id).await? && key.user_id != user_id {
            return Err(AppError::Forbidden);
        }

        repos.webdav_key.delete_by_id(key_id).await
    }
}
