//! Password reset service for UI layer refactoring.
//!
//! This service handles password reset token creation and validation.

use std::sync::Arc;

use rand::Rng;
use sha2::Digest;

use crate::repository::Repositories;
use crate::service::auth::password::hash_password;
use base::error::AppError;
use infra::entity::password_reset_token;

/// Result of password reset token creation.
pub struct PasswordResetTokenResult {
    pub raw_token: String,
    pub reset_url: Option<String>,
}

/// Service handling password reset operations.
pub struct PasswordResetService {
    repos: Arc<Repositories>,
    /// In-memory capability-URL manager, so a reset also drops the user's
    /// outstanding upload/download tokens. `None` in contexts without one.
    token_manager: Option<Arc<crate::AccessTokenManager>>,
}

impl PasswordResetService {
    pub fn new(repos: Arc<Repositories>) -> Self {
        Self {
            repos,
            token_manager: None,
        }
    }

    /// Same as [`Self::new`], but able to revoke in-memory capability URLs on
    /// reset as well.
    pub fn with_token_manager(
        repos: Arc<Repositories>,
        token_manager: Arc<crate::AccessTokenManager>,
    ) -> Self {
        Self {
            repos,
            token_manager: Some(token_manager),
        }
    }

    /// Create a password reset token for a user.
    ///
    /// Returns None if user is not found (to prevent enumeration).
    pub async fn create_reset_token(
        &self,
        email: &str,
        site_url: &str,
    ) -> Result<PasswordResetTokenResult, AppError> {
        // Look up the user
        let user_record = self.repos.user.find_by_email(email).await?;

        let reset_url = if let Some(user) = user_record {
            let now = chrono::Utc::now().timestamp();
            let (raw_token, token_hash) = generate_reset_token();

            // Invalidate any previously issued reset links so old tokens stop
            // working as soon as a new one is requested.
            let old_tokens = self
                .repos
                .password_reset_token
                .find_by_user(user.id)
                .await?;
            for old in old_tokens {
                self.repos.password_reset_token.delete_by_id(old.id).await?;
            }

            self.repos
                .password_reset_token
                .create(user.id, token_hash, now, now + RESET_TOKEN_TTL_SECONDS)
                .await?;

            let base = site_url.trim_end_matches('/');
            let link = format!("{}/accounts/password/reset/{}/", base, raw_token);
            tracing::info!("Password reset link generated for user {}", user.email);
            Some(link)
        } else {
            tracing::info!("Password reset requested for unknown email: {}", email);
            None
        };

        Ok(PasswordResetTokenResult {
            raw_token: String::new(), // Not needed by caller
            reset_url,
        })
    }

    /// Validate a password reset token and return the token record if valid.
    pub async fn validate_token(
        &self,
        raw_token: &str,
    ) -> Result<Option<password_reset_token::Model>, AppError> {
        let token_hash = hash_token(raw_token);
        let record = self
            .repos
            .password_reset_token
            .find_by_token_hash(&token_hash)
            .await?;

        let record = match record {
            Some(r) => r,
            None => return Ok(None),
        };

        let now = chrono::Utc::now().timestamp();

        if record.used || record.expires_at <= now {
            return Ok(None);
        }

        Ok(Some(record))
    }

    /// Complete the password reset process.
    ///
    /// Claim the token **first**, then set the password: the claim is a single
    /// conditional UPDATE, so two concurrent submissions of the same link
    /// cannot both take effect (the loser aborts before writing a password).
    pub async fn reset_password(
        &self,
        raw_token: &str,
        new_password: &str,
        password_min_length: usize,
        require_strong_password: bool,
        password_hash_iterations: u32,
    ) -> Result<(), AppError> {
        // Validate token
        let record = self.validate_token(raw_token).await?;
        let record = match record {
            Some(r) => r,
            None => {
                return Err(AppError::BadRequest(
                    "This reset link is invalid or has expired.".to_string(),
                ));
            }
        };

        // Validate password before claiming, so a weak password does not burn
        // the link.
        crate::service::auth::password::validate_password(
            new_password,
            password_min_length as u32,
            require_strong_password,
        )
        .map_err(AppError::BadRequest)?;

        // Claim the token atomically. Losing the race means someone else is
        // resetting with the same link, so this request must not write a
        // password — the last writer would otherwise win.
        if !self
            .repos
            .password_reset_token
            .mark_as_used(record.id)
            .await?
        {
            return Err(AppError::BadRequest(
                "This reset link is invalid or has expired.".to_string(),
            ));
        }

        // Update password
        let password_hash = hash_password(new_password, password_hash_iterations);
        self.repos
            .user
            .update_password(record.user_id, password_hash)
            .await?;

        // Revoke every credential (sessions, API tokens, 2FA device trust,
        // repository sync tokens and any outstanding reset links) so a stolen
        // credential cannot outlive the reset — matching seahub's
        // `clear_token()` on password reset.
        crate::service::auth::token::revoke_all_credentials(
            &self.repos,
            self.token_manager.as_ref(),
            record.user_id,
            None,
        )
        .await?;

        Ok(())
    }
}

// ── Token generation helpers (from auth/password_reset.rs) ────────────────

/// Generate a raw reset token and its SHA-256 hash.
/// Returns (raw_token, token_hash).
pub fn generate_reset_token() -> (String, String) {
    let mut raw = [0u8; 32];
    rand::rng().fill_bytes(&mut raw);
    let raw_token = hex::encode(raw);
    let hash = hash_token(&raw_token);
    (raw_token, hash)
}

/// Compute the SHA-256 hash of a raw token for database storage.
pub fn hash_token(token: &str) -> String {
    let mut hasher = sha2::Sha256::new();
    hasher.update(token.as_bytes());
    hex::encode(hasher.finalize())
}

/// Token expiry: 3 days (matching seahub).
pub const RESET_TOKEN_TTL_SECONDS: i64 = 3 * 24 * 60 * 60;
