use rand::Rng;
use sha2::{Digest, Sha256};

use base::error::AppError;

const TOKEN_LEN: usize = 40;

/// Return the existing sync token for a repo/user, or create a new one.
///
/// Requires read permission on the repo (a sync token grants repo access).
pub async fn ensure_sync_token(
    repos: &crate::repository::Repositories,
    repo_id: &str,
    user_id: i32,
    sync_token_ttl_days: u64,
) -> Result<String, AppError> {
    crate::domain::permission::check_repo_read_permission(repos.member.as_ref(), repo_id, user_id)
        .await?;

    let now = chrono::Utc::now().timestamp();

    // Reuse an existing non-expired token; replace an expired one.
    if let Some(existing) = repos
        .sync_token
        .find_by_repo_and_user(repo_id, user_id)
        .await?
    {
        let expired = existing.expires_at.is_some_and(|exp| now > exp);
        if !expired {
            return Ok(existing.token);
        }
        repos.sync_token.delete_by_token(&existing.token).await?;
    }

    let token_value = generate_sync_token();
    let expires_at = (sync_token_ttl_days > 0).then(|| now + sync_token_ttl_days as i64 * 86400);
    repos
        .sync_token
        .create(repo_id, user_id, token_value.clone(), None, now, expires_at)
        .await?;
    Ok(token_value)
}

pub fn generate_api_token() -> String {
    let mut token = [0u8; TOKEN_LEN / 2];
    rand::rng().fill_bytes(&mut token);
    hex::encode(token)
}

pub fn generate_sync_token() -> String {
    let mut token = [0u8; TOKEN_LEN / 2];
    rand::rng().fill_bytes(&mut token);
    hex::encode(token)
}

pub fn generate_share_link_token() -> String {
    let mut token = [0u8; 16];
    rand::rng().fill_bytes(&mut token);
    base64::Engine::encode(&base64::engine::general_purpose::URL_SAFE_NO_PAD, token)
}

pub fn generate_upload_link_token() -> String {
    let mut token = [0u8; 16];
    rand::rng().fill_bytes(&mut token);
    base64::Engine::encode(&base64::engine::general_purpose::URL_SAFE_NO_PAD, token)
}

pub fn generate_backup_code() -> String {
    // 80 bits of entropy — high enough that even with a fast SHA-256 hash,
    // offline brute-force against a leaked database is infeasible.
    let mut code = [0u8; 10];
    rand::rng().fill_bytes(&mut code);
    hex::encode(code).to_uppercase()
}

/// Compute the SHA-256 hash of a bearer token for database storage.
///
/// Tokens are 160-bit random values, so a fast hash (not a slow KDF) is used —
/// the purpose is only to prevent an attacker who reads the database from
/// obtaining the raw token, which is infeasible to invert at that entropy.
pub fn hash_token(token: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(token.as_bytes());
    hex::encode(hasher.finalize())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_hash_token_hex_length() {
        let h = hash_token("some-token-value");
        assert_eq!(h.len(), 64);
        assert!(h.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn test_hash_token_deterministic_and_distinct() {
        let a = hash_token("token-a");
        assert_eq!(a, hash_token("token-a"));
        assert_ne!(a, hash_token("token-b"));
    }

    #[test]
    fn test_generate_backup_code_entropy() {
        let code = generate_backup_code();
        // 10 bytes → 20 hex chars = 80 bits of entropy.
        assert_eq!(code.len(), 20);
        assert!(code.chars().all(|c| c.is_ascii_hexdigit()));
    }
}
