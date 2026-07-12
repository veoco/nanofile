use std::sync::Arc;

use crate::auth::backup_codes::BackupCodeManager;
use crate::auth::password::verify_password;
use crate::auth::totp::TotpManager;
use crate::repository::Repositories;
use base::error::AppError;
use infra::rate_limit::GenericRateLimiter;

/// Result from a successful 2FA setup.
pub struct Setup2faResult {
    pub secret: String,
    pub otpauth_url: String,
    pub backup_codes: Vec<String>,
}

/// Service for two-factor authentication setup, verification, and disabling.
pub struct TwoFactorService {
    repos: Arc<Repositories>,
    password_hash_iterations: u32,
    disable_2fa_limiter: Arc<GenericRateLimiter>,
}

impl TwoFactorService {
    pub fn new(
        repos: Arc<Repositories>,
        password_hash_iterations: u32,
        disable_2fa_limiter: Arc<GenericRateLimiter>,
    ) -> Self {
        Self {
            repos,
            password_hash_iterations,
            disable_2fa_limiter,
        }
    }

    /// Set up 2FA for a user. Creates or retrieves the existing TOTP secret,
    /// returns the secret, otpauth URL, and backup codes.
    pub async fn setup_2fa(&self, user_id: i32, email: &str) -> Result<Setup2faResult, AppError> {
        let model = self
            .repos
            .user_2fa
            .get_or_create(user_id, TotpManager::generate_secret())
            .await?;

        let totp = TotpManager::create_totp(&model.totp_secret, email, "Nanofile")
            .map_err(|e| AppError::Internal(e.to_string()))?;

        let backup_codes = BackupCodeManager::generate_codes(10);
        BackupCodeManager::store_codes(&self.repos, user_id, &backup_codes).await?;

        Ok(Setup2faResult {
            secret: model.totp_secret,
            otpauth_url: TotpManager::get_otpauth_url(&totp),
            backup_codes,
        })
    }

    /// Verify a TOTP code and enable 2FA for the user.
    pub async fn verify_2fa(&self, user_id: i32, email: &str, code: &str) -> Result<(), AppError> {
        let model = self
            .repos
            .user_2fa
            .find_by_user_id(user_id)
            .await?
            .ok_or_else(|| AppError::BadRequest("2FA not set up".into()))?;

        let totp = TotpManager::create_totp(&model.totp_secret, email, "Nanofile")
            .map_err(|e| AppError::Internal(e.to_string()))?;

        if TotpManager::verify_code(&totp, code) {
            let now = chrono::Utc::now().timestamp();
            self.repos.user_2fa.set_enabled(user_id, true, now).await?;
            Ok(())
        } else {
            Err(AppError::TwoFactorInvalid)
        }
    }

    /// Disable 2FA for a user. Validates the password and rate-limits attempts.
    pub async fn disable_2fa(&self, user_id: i32, password: &str) -> Result<(), AppError> {
        // Rate limit password attempts on 2FA disable
        let rate_limit_key = format!("2fa_disable:{user_id}");
        if self.disable_2fa_limiter.is_limited(&rate_limit_key) {
            return Err(AppError::TooManyRequests);
        }
        self.disable_2fa_limiter.record_attempt(&rate_limit_key);

        let user_record = self
            .repos
            .user
            .find_by_id(user_id)
            .await?
            .ok_or(AppError::Unauthorized)?;

        if !verify_password(
            password,
            &user_record.password_hash,
            self.password_hash_iterations,
        ) {
            return Err(AppError::Unauthorized);
        }

        self.repos
            .user_2fa
            .set_enabled(user_id, false, chrono::Utc::now().timestamp())
            .await?;

        // Clear rate limit on successful disable
        self.disable_2fa_limiter.clear(&rate_limit_key);

        Ok(())
    }
}
