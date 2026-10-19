use rand::Rng;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::sync::Arc;

use base::error::AppError;
use infra::entity::sync_token;

use crate::domain::device::PeerStamp;
use crate::repository::Repositories;

const TOKEN_LEN: usize = 40;

/// Verify a sync token that the client supplied in the request **body**.
///
/// `folder-perm` and `locked-files` carry the token in JSON rather than a header,
/// so they never go through `SyncAuth` and must apply the same rules here: the
/// token must belong to the repo, be unexpired, and belong to an active user.
/// Checking only "the row exists" would let an expired or deactivated token keep
/// working until the hourly cleanup.
pub async fn verify_body_sync_token(
    repos: &crate::repository::Repositories,
    repo_id: &str,
    token: &str,
) -> Result<bool, AppError> {
    let Some(record) = repos
        .sync_token
        .find_by_token_and_repo(token, repo_id)
        .await?
    else {
        return Ok(false);
    };

    if matches!(record.expires_at, Some(exp) if chrono::Utc::now().timestamp() > exp) {
        return Ok(false);
    }

    Ok(repos
        .user
        .find_by_id(record.user_id)
        .await?
        .is_some_and(|u| u.is_active && u.email_verified))
}

/// Return the sync token the caller should use for one repository, or create it.
/// Requires read permission on the repo (a sync token grants repo access).
pub async fn ensure_sync_token_for(
    repos: &Repositories,
    repo_id: &str,
    user_id: i32,
    peer: Option<&PeerStamp>,
    sync_token_ttl_days: u64,
) -> Result<String, AppError> {
    crate::domain::permission::check_repo_read_permission(repos.member.as_ref(), repo_id, user_id)
        .await?;

    let mut tokens = ensure_sync_tokens_for_repos(
        repos,
        std::slice::from_ref(&repo_id.to_string()),
        user_id,
        peer,
        sync_token_ttl_days,
    )
    .await?;

    tokens
        .remove(repo_id)
        .ok_or_else(|| AppError::internal("sync token was not resolved"))
}

/// Resolve the sync token of many repositories at once.
///
/// A token belongs to one `(repository, user, device)`; the caller has already
/// checked read permission for every repo. Per repo: the device's own token is
/// reused; failing that, an *unattributed* token (minted by a caller with no device
/// identity) is claimed for this device; failing that, a fresh token is minted with
/// the device already recorded. A caller with no device shares the single
/// unattributed token per repository, as it always has. The two loads are batched,
/// so syncing twenty libraries costs two queries rather than forty.
pub async fn ensure_sync_tokens_for_repos(
    repos: &Repositories,
    repo_ids: &[String],
    user_id: i32,
    peer: Option<&PeerStamp>,
    sync_token_ttl_days: u64,
) -> Result<HashMap<String, String>, AppError> {
    if repo_ids.is_empty() {
        return Ok(HashMap::new());
    }

    let peer_id = peer.map(|p| p.id.as_str());
    let own: HashMap<String, sync_token::Model> = repos
        .sync_token
        .find_by_repos_user_peer(repo_ids, user_id, peer_id)
        .await?
        .into_iter()
        .map(|row| (row.repo_id.clone(), row))
        .collect();
    let unattributed: HashMap<String, sync_token::Model> = repos
        .sync_token
        .find_by_repos_user_peer(repo_ids, user_id, None)
        .await?
        .into_iter()
        .map(|row| (row.repo_id.clone(), row))
        .collect();

    let now = chrono::Utc::now().timestamp();
    let mut tokens = HashMap::with_capacity(repo_ids.len());
    for repo_id in repo_ids {
        let token = resolve_repo_token(
            repos,
            repo_id,
            user_id,
            peer,
            own.get(repo_id),
            unattributed.get(repo_id),
            now,
            sync_token_ttl_days,
        )
        .await?;
        tokens.insert(repo_id.clone(), token);
    }
    Ok(tokens)
}

/// The token for one repository given the already loaded rows; see
/// [`ensure_sync_tokens_for_repos`] for the rules.
#[allow(clippy::too_many_arguments)]
async fn resolve_repo_token(
    repos: &Repositories,
    repo_id: &str,
    user_id: i32,
    peer: Option<&PeerStamp>,
    own: Option<&sync_token::Model>,
    unattributed: Option<&sync_token::Model>,
    now: i64,
    sync_token_ttl_days: u64,
) -> Result<String, AppError> {
    if let Some(row) = own {
        if let Some(raw) = reusable(repos, row, now) {
            return Ok(raw);
        }
        repos.sync_token.delete_by_id(row.id).await?;
    }

    if let Some(row) = unattributed {
        if let Some(raw) = reusable(repos, row, now) {
            match peer {
                None => return Ok(raw),
                Some(peer) => {
                    if repos.sync_token.attach_peer_if_unset(row.id, peer).await? {
                        return Ok(raw);
                    }
                    // A concurrent request attached it first. If it became ours (a
                    // retry of our request), reuse it; otherwise it belongs to
                    // another device and we mint our own.
                    if let Some(mine) = repos
                        .sync_token
                        .find_by_repo_user_peer(repo_id, user_id, Some(&peer.id))
                        .await?
                        && let Some(raw) = reusable(repos, &mine, now)
                    {
                        return Ok(raw);
                    }
                }
            }
        } else {
            // Expired or undecryptable. Only a still-unattributed row may be
            // dropped: one that was claimed meanwhile belongs to someone else.
            let still_unattributed = repos
                .sync_token
                .find_by_repo_user_peer(repo_id, user_id, None)
                .await?
                .is_some();
            if still_unattributed {
                repos.sync_token.delete_by_id(row.id).await?;
            }
        }
    }

    let token_value = generate_sync_token();
    let expires_at = (sync_token_ttl_days > 0).then(|| now + sync_token_ttl_days as i64 * 86400);
    if let Err(error) = repos
        .sync_token
        .create(repo_id, user_id, token_value.clone(), peer, now, expires_at)
        .await
    {
        // Two concurrent requests may both reach here; the unique `(repo, user, peer)`
        // index lets only one insert through, and the loser uses the winner's row.
        if let Some(peer) = peer
            && let Some(mine) = repos
                .sync_token
                .find_by_repo_user_peer(repo_id, user_id, Some(&peer.id))
                .await?
            && let Some(raw) = reusable(repos, &mine, now)
        {
            return Ok(raw);
        }
        return Err(error);
    }
    Ok(token_value)
}

/// The raw value of a row that is still usable, or `None` when expired or its
/// ciphertext cannot be decrypted (the server secret changed).
fn reusable(repos: &Repositories, row: &sync_token::Model, now: i64) -> Option<String> {
    if row.expires_at.is_some_and(|expires| now > expires) {
        return None;
    }
    repos.sync_token.reveal_token(row)
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
/// Tokens are 160-bit random values, so a fast hash suffices — the purpose is only
/// to keep raw tokens out of a leaked database, which is infeasible to invert.
pub fn hash_token(token: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(token.as_bytes());
    hex::encode(hasher.finalize())
}

/// Revoke every credential for a user, optionally keeping the acting session.
///
/// Mirrors seahub's `clear_token()` on password change/reset: API tokens, unified
/// API keys (WebDAV keys included), 2FA device-trust tokens, **all** repository
/// sync tokens and outstanding reset links are dropped, so a stolen credential
/// cannot survive the remediation. `keep_session_token` preserves the acting
/// session (seahub's `update_session_auth_hash`).
///
/// `token_manager` additionally drops this user's in-memory one-hour capability
/// URLs (`/download-api/…`, `/upload-api/…`, `/blks/…`), so a leaked URL does not
/// outlive the password change. Pass `None` only where no manager is reachable.
pub async fn revoke_all_credentials(
    repos: &crate::repository::Repositories,
    token_manager: Option<&Arc<crate::AccessTokenManager>>,
    user_id: i32,
    keep_session_token: Option<&str>,
) -> Result<(), AppError> {
    match keep_session_token {
        Some(token) => {
            repos
                .api_token
                .delete_many_by_user_id_except(user_id, token)
                .await?;
        }
        None => repos.api_token.delete_many_by_user_id(user_id).await?,
    }
    repos.s2fa_token.delete_by_user(user_id).await?;
    repos.sync_token.delete_by_user(user_id).await?;
    // Unified API keys include the WebDAV-scoped ones, which live in their own
    // table with no TTL.
    repos.api_key.delete_by_user(user_id).await?;

    // Outstanding reset links are credentials too: one that survives a password
    // change keeps offering account takeover for the rest of its 3-day TTL.
    for token in repos.password_reset_token.find_by_user(user_id).await? {
        repos.password_reset_token.delete_by_id(token.id).await?;
    }

    if let Some(manager) = token_manager {
        let revoked = manager.revoke_user(user_id);
        if revoked > 0 {
            tracing::debug!(user_id, revoked, "revoked in-memory access tokens");
        }
    }

    Ok(())
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
        assert_eq!(code.len(), 20);
        assert!(code.chars().all(|c| c.is_ascii_hexdigit()));
    }
}
