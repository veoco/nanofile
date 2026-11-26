//! One interpretation of a presented unified API key.
//!
//! The REST extractor, the sync extractor and WebDAV each looked up an API key and
//! then repeated "reject when expired, reject when the capability string cannot be
//! decoded, note last-used". WebDAV additionally had its own hourly `last_used_at`
//! writer. This module owns that sequence; callers keep their transport-specific
//! status mapping (401 for REST/WebDAV, 403 for sync) and their scope checks.

use crate::AppState;
use crate::domain::api_key::ApiKeyLookup;
use crate::domain::api_key::KeyAuthority;

/// Why a presented key is not usable.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ApiKeyFailure {
    /// No row matches the presented secret, or it was revoked.
    Unknown,
    /// The row's `expires_at` has passed.
    Expired,
    /// The stored capability string cannot be decoded by this build.
    Unreadable,
    /// The lookup itself failed (database error).
    Internal,
}

/// A successfully authenticated API key.
pub(crate) struct ApiKeySession {
    pub user_id: i32,
    pub key_id: i32,
    pub authority: KeyAuthority,
}

/// Interpret an already-fetched key row.
///
/// Exposed separately so callers that already join the lookup with other
/// credential lookups keep that concurrency. Callers record usage themselves
/// with [`note_key_usage`] once the request is authenticated.
pub(crate) fn session_from_lookup(lookup: ApiKeyLookup) -> Result<ApiKeySession, ApiKeyFailure> {
    if crate::domain::token::is_token_expired(lookup.key.expires_at) {
        return Err(ApiKeyFailure::Expired);
    }
    let authority = KeyAuthority::from_lookup(&lookup).map_err(|error| {
        // An unreadable grant means this build cannot honour what the row
        // promises; refusing the credential is the safe direction.
        tracing::warn!(
            key_id = lookup.key.id,
            %error,
            "rejecting API key with an unreadable capability set"
        );
        ApiKeyFailure::Unreadable
    })?;
    let key_id = lookup.key.id;
    Ok(ApiKeySession {
        user_id: lookup.key.user_id,
        key_id,
        authority,
    })
}

/// Look up and interpret a presented API key (WebDAV's path; no other
/// credential kind can match the same secret).
pub(crate) async fn authenticate_api_key(
    state: &AppState,
    presented: &str,
) -> Result<ApiKeySession, ApiKeyFailure> {
    let lookup = state
        .repos
        .api_key
        .find_by_presented(presented)
        .await
        .map_err(|error| {
            tracing::warn!(%error, "API key lookup failed");
            ApiKeyFailure::Internal
        })?
        .ok_or(ApiKeyFailure::Unknown)?;
    session_from_lookup(lookup)
}

/// Record that a key authenticated a request, at most once per
/// [`crate::repository::throttle::KEY_USAGE_WRITE_INTERVAL`]; best effort, and
/// it counts authentication rather than success.
pub(crate) async fn note_key_usage(repos: &crate::repository::Repositories, key_id: i32) {
    if !repos.key_usage_writes.allows_now(key_id) {
        return;
    }
    if let Err(error) = repos
        .api_key
        .touch_last_used(key_id, chrono::Utc::now().timestamp())
        .await
    {
        tracing::debug!(key_id, %error, "failed to record API key usage");
    }
}
