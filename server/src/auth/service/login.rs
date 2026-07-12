use std::sync::Arc;

use sea_orm::Set;

use crate::auth::password::verify_password;
use crate::auth::s2fa::{S2FA_TTL_SECONDS, generate_s2fa_token};
use crate::auth::token::generate_api_token;
use crate::auth::totp::TotpManager;
use crate::repository::Repositories;
use base::error::AppError;
use infra::entity::{api_token, s2fa_token};
use infra::rate_limit::LoginRateLimiter;

/// Represents all possible outcomes of a login attempt.
pub enum LoginResult {
    /// Login succeeded. Includes the API token and an optional S2FA device trust token.
    Success {
        api_token: String,
        s2fa_token: Option<String>,
    },
    /// Rate limited — too many failed attempts.
    RateLimited,
    /// Invalid username or password.
    BadCredentials,
    /// User account is disabled.
    AccountDisabled,
    /// The user has 2FA enabled and needs to provide an OTP.
    TwoFactorRequired,
    /// The user provided an OTP, but it was invalid.
    TwoFactorInvalid,
}

/// Service handling login authentication including rate limiting
/// and two-factor / S2FA device trust flows.
pub struct LoginService {
    repos: Arc<Repositories>,
    password_hash_iterations: u32,
    api_token_ttl_days: u64,
    login_rate_limiter: Arc<LoginRateLimiter>,
}

impl LoginService {
    pub fn new(
        repos: Arc<Repositories>,
        password_hash_iterations: u32,
        api_token_ttl_days: u64,
        login_rate_limiter: Arc<LoginRateLimiter>,
    ) -> Self {
        Self {
            repos,
            password_hash_iterations,
            api_token_ttl_days,
            login_rate_limiter,
        }
    }

    /// Attempt to authenticate a user with the given credentials.
    ///
    /// Arguments:
    /// - `username`: the user's email / login name
    /// - `password`: the plaintext password
    /// - `client_ip`: IP address for rate-limit key scoping
    /// - `s2fa_header`: optional `X-Seafile-S2FA` header (device trust token)
    /// - `otp`: optional `X-Seafile-OTP` header (TOTP code)
    /// - `trust_device`: whether the client requested a device trust token
    /// - `platform`, `device_id`, `device_name`, `client_version`: device metadata stored with the API token
    pub async fn authenticate(
        &self,
        username: &str,
        password: &str,
        client_ip: &str,
        s2fa_header: Option<&str>,
        otp: Option<&str>,
        trust_device: bool,
        platform: Option<String>,
        device_id: Option<String>,
        device_name: Option<String>,
        client_version: Option<String>,
    ) -> Result<LoginResult, AppError> {
        // ── Rate limit keys ──────────────────────────────────────────────
        let rate_limit_key_ip = format!("login:ip:{client_ip}");
        let rate_limit_key_user = format!("login:user:{username}");
        let rate_limit_key_global = "login:global".to_string();

        if self.login_rate_limiter.is_locked(&rate_limit_key_ip)
            || self.login_rate_limiter.is_locked(&rate_limit_key_user)
            || self.login_rate_limiter.is_locked(&rate_limit_key_global)
        {
            return Ok(LoginResult::RateLimited);
        }

        // ── Record failure helper ─────────────────────────────────────────
        let record_failure = || {
            self.login_rate_limiter.record_failure(&rate_limit_key_ip);
            self.login_rate_limiter.record_failure(&rate_limit_key_user);
            self.login_rate_limiter
                .record_failure(&rate_limit_key_global);
        };

        // ── Find user ─────────────────────────────────────────────────────
        let user_record = match self.repos.user.find_by_email(username).await? {
            Some(u) => u,
            None => {
                record_failure();
                return Ok(LoginResult::BadCredentials);
            }
        };

        // ── Verify password ───────────────────────────────────────────────
        if !verify_password(
            password,
            &user_record.password_hash,
            self.password_hash_iterations,
        ) {
            record_failure();
            return Ok(LoginResult::BadCredentials);
        }

        // ── Check if user is active ───────────────────────────────────────
        if !user_record.is_active {
            record_failure();
            return Ok(LoginResult::AccountDisabled);
        }

        // ── Two-factor authentication ──────────────────────────────────────
        let mut skip_2fa = false;
        let mut issued_s2fa_token: Option<String> = None;

        let two_fa = self.repos.user_2fa.find_by_user_id(user_record.id).await?;

        if let Some(tfa) = two_fa
            && tfa.enabled
        {
            // --- S2FA device trust token check ---
            if let Some(s2fa_token_val) = s2fa_header {
                let now = chrono::Utc::now().timestamp();
                self.repos
                    .s2fa_token
                    .delete_expired(user_record.id, now)
                    .await?;

                let stored = self
                    .repos
                    .s2fa_token
                    .find_by_token_and_user(s2fa_token_val, user_record.id)
                    .await?;

                if let Some(stored) = stored
                    && stored.expires_at > now
                {
                    skip_2fa = true;
                }
            }

            if !skip_2fa {
                match otp {
                    Some(otp_code) => {
                        let totp =
                            TotpManager::create_totp(&tfa.totp_secret, &user_record.email, "")
                                .map_err(|e| AppError::Internal(e.to_string()))?;

                        if !TotpManager::verify_code(&totp, otp_code) {
                            record_failure();
                            return Ok(LoginResult::TwoFactorInvalid);
                        }

                        if trust_device {
                            let s2fa_token_value = generate_s2fa_token();
                            let now = chrono::Utc::now().timestamp();
                            let s2fa_model = s2fa_token::ActiveModel {
                                id: sea_orm::NotSet,
                                user_id: Set(user_record.id),
                                token: Set(s2fa_token_value.clone()),
                                device_id: Set(device_id.clone()),
                                device_name: Set(device_name.clone()),
                                created_at: Set(now),
                                expires_at: Set(now + S2FA_TTL_SECONDS),
                            };
                            self.repos.s2fa_token.insert(s2fa_model).await?;
                            issued_s2fa_token = Some(s2fa_token_value);
                        }
                    }
                    None => {
                        return Ok(LoginResult::TwoFactorRequired);
                    }
                }
            }
        }

        // ── Login succeeded — create API token ─────────────────────────────
        self.login_rate_limiter.clear(&rate_limit_key_ip);
        self.login_rate_limiter.clear(&rate_limit_key_user);
        self.login_rate_limiter.clear(&rate_limit_key_global);

        let token_value = generate_api_token();
        let now = chrono::Utc::now().timestamp();

        let token_model = api_token::ActiveModel {
            id: sea_orm::NotSet,
            user_id: Set(user_record.id),
            token: Set(token_value.clone()),
            created_at: Set(now),
            expires_at: Set(if self.api_token_ttl_days > 0 {
                Some(now + self.api_token_ttl_days as i64 * 86400)
            } else {
                None
            }),
            device_id: Set(device_id),
            platform: Set(platform),
            device_name: Set(device_name),
            client_version: Set(client_version),
        };
        self.repos.api_token.insert(token_model).await?;

        self.repos
            .user
            .touch_last_login(user_record.id, now)
            .await?;

        Ok(LoginResult::Success {
            api_token: token_value,
            s2fa_token: issued_s2fa_token,
        })
    }
}
