//! Password reset service for UI layer refactoring.
//!
//! This service handles password reset token creation and validation.

use std::sync::Arc;

use crate::auth::password::hash_password;
use crate::auth::password_reset::{generate_reset_token, hash_token, RESET_TOKEN_TTL_SECONDS};
use crate::entity::password_reset_token;
use crate::error::AppError;
use crate::repository::Repositories;

/// Result of password reset token creation.
pub struct PasswordResetTokenResult {
    pub raw_token: String,
    pub reset_url: Option<String>,
}

/// Service handling password reset operations.
pub struct PasswordResetService {
    repos: Arc<Repositories>,
}

impl PasswordResetService {
    pub fn new(repos: Arc<Repositories>) -> Self {
        Self { repos }
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

            self.repos
                .password_reset_token
                .create(
                    user.id,
                    token_hash,
                    now,
                    now + RESET_TOKEN_TTL_SECONDS,
                )
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
    /// Validates the token, updates the password, and marks the token as used.
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

        // Validate password
        crate::auth::password::validate_password(
            new_password,
            password_min_length as u32,
            require_strong_password,
        )
        .map_err(AppError::BadRequest)?;

        // Update password
        let password_hash = hash_password(new_password, password_hash_iterations);
        self.repos
            .user
            .update_password(record.user_id, password_hash)
            .await?;

        // Mark token as used
        self.repos
            .password_reset_token
            .mark_as_used(record.id)
            .await?;

        // Delete all session tokens for this user (force re-login)
        self.repos
            .api_token
            .delete_many_by_user_id(record.user_id)
            .await?;

        Ok(())
    }
}