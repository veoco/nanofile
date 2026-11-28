//! Capability-token validation shared by the `/upload-api`, `/update-api`,
//! `/files/{token}/{filename}` and `/upload-blks-api` token families.
//!
//! Each family used to hand-roll "look up the token, compare `op`, check the
//! upload-link deadline, check the account is active". The copies had drifted:
//! `block_auth` skipped the link deadline, and no upload/update token path checked
//! the account at all, so a deactivated account kept its outstanding capability
//! URLs until they expired. The family-specific permission and encryption checks
//! stay at the call sites; everything that must be true for *every* capability
//! token lives here.

use crate::AppState;
use crate::service::auth::access_token::AccessToken;
use base::error::AppError;

/// What one capability-token family accepts.
pub(crate) struct TokenPolicy<'a> {
    /// Token operations this endpoint accepts.
    pub ops: &'a [&'static str],
    /// Message for a token that does not resolve (family wording is wire-visible).
    pub invalid_message: &'static str,
    /// Message for a token of the wrong family.
    pub wrong_op_message: &'static str,
    /// Whether the token minted from an upload link must outlive the link itself.
    pub check_upload_link_expiry: bool,
}

/// Validate an in-memory capability token against a family policy.
///
/// Checks, in order: token exists/not expired, operation is allowed, the
/// upload-link deadline (when the family carries one) and the owner account is
/// still active. Missing account and deactivated/unverified account both surface
/// as 403, matching the check the capability endpoints already applied.
pub(crate) async fn validate_capability(
    state: &AppState,
    raw: &str,
    policy: TokenPolicy<'_>,
) -> Result<AccessToken, AppError> {
    let token = state
        .token_manager
        .validate(raw)
        .ok_or_else(|| AppError::BadRequest(policy.invalid_message.into()))?;

    if !policy.ops.contains(&token.op.as_str()) {
        return Err(AppError::BadRequest(policy.wrong_op_message.into()));
    }

    // An upload-link token must not outlive the link it was minted from: the
    // token's own TTL is one hour, while the link may expire (or have been
    // shortened) before that. The deadline is captured at mint time, so this
    // needs no extra query.
    if policy.check_upload_link_expiry
        && let Some(expires_at) = token.upload_link_expires_at
        && chrono::Utc::now().timestamp() > expires_at
    {
        return Err(AppError::BadRequest("upload link has expired".into()));
    }

    if !crate::service::auth::account_state::is_active_account(&state.repos, token.user_id).await? {
        return Err(AppError::Forbidden);
    }

    Ok(token)
}
