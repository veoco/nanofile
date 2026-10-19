//! Email ownership proof for open self-registration.
//!
//! A link carries a random one-time token; only its SHA-256 is stored. The token
//! is claimed with a conditional update, then the user row is marked verified.
//! Issuing a new link removes the previous one, so a resend cannot leave two live
//! links behind.

use std::sync::Arc;

use crate::repository::Repositories;
use crate::service::auth::password_reset::{generate_reset_token, hash_token};
use base::error::AppError;

/// Verification links live as long as password-reset links.
pub const EMAIL_VERIFICATION_TOKEN_TTL_SECS: i64 = 3 * 24 * 60 * 60;

/// A second link is not minted immediately after the first: a user who signs in
/// right after registering must not invalidate the mail that is already on its
/// way. The page shown either way is the same.
pub const EMAIL_VERIFICATION_RESEND_COOLDOWN_SECS: i64 = 5 * 60;

/// How long an account that never verified its address is kept. Open
/// registration must not let an anonymous caller grow the users table forever.
pub const UNVERIFIED_ACCOUNT_TTL_SECS: i64 = 7 * 24 * 60 * 60;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VerificationOutcome {
    Verified { user_id: i32 },
    Invalid,
}

pub struct EmailVerificationService {
    repos: Arc<Repositories>,
}

impl EmailVerificationService {
    pub fn new(repos: Arc<Repositories>) -> Self {
        Self { repos }
    }

    /// Mint a link for `user_id`, replacing any previous one, and return the
    /// absolute URL. The raw token exists only in this return value (and the mail
    /// queued from it); the database holds its hash.
    pub async fn issue(&self, user_id: i32, site_url: &str) -> Result<String, AppError> {
        let now = chrono::Utc::now().timestamp();
        let (raw_token, token_hash) = generate_reset_token();

        self.repos
            .email_verification_token
            .delete_by_user(user_id)
            .await?;
        self.repos
            .email_verification_token
            .create(
                user_id,
                token_hash,
                now,
                now + EMAIL_VERIFICATION_TOKEN_TTL_SECS,
            )
            .await?;

        let base = site_url.trim_end_matches('/');
        Ok(format!("{base}/accounts/verify-email/{raw_token}/"))
    }

    /// Whether a link was issued recently enough that another send would only
    /// invalidate the one already in flight.
    pub async fn recently_issued(&self, user_id: i32, now: i64) -> Result<bool, AppError> {
        Ok(self
            .repos
            .email_verification_token
            .last_created_at(user_id)
            .await?
            .is_some_and(|created| now - created < EMAIL_VERIFICATION_RESEND_COOLDOWN_SECS))
    }

    /// Claim a link and mark its account verified.
    ///
    /// The claim is the single-use gate: two concurrent visits of the same link
    /// cannot both succeed. An unknown, used or expired token is [`Invalid`]
    /// rather than an error, because the page is the same for all of them.
    pub async fn verify(&self, raw_token: &str) -> Result<VerificationOutcome, AppError> {
        let token_hash = hash_token(raw_token);
        let Some(record) = self
            .repos
            .email_verification_token
            .find_by_token_hash(&token_hash)
            .await?
        else {
            return Ok(VerificationOutcome::Invalid);
        };

        let now = chrono::Utc::now().timestamp();
        if record.used || record.expires_at <= now {
            return Ok(VerificationOutcome::Invalid);
        }

        // A token whose user was deleted (for instance by the stale-account
        // cleanup) must not resurrect anything.
        if self.repos.user.find_by_id(record.user_id).await?.is_none() {
            return Ok(VerificationOutcome::Invalid);
        }

        if !self
            .repos
            .email_verification_token
            .mark_as_used(record.id)
            .await?
        {
            return Ok(VerificationOutcome::Invalid);
        }

        self.repos.user.mark_email_verified(record.user_id).await?;
        Ok(VerificationOutcome::Verified {
            user_id: record.user_id,
        })
    }
}
