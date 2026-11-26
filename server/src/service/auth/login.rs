use std::sync::Arc;

use crate::domain::session_source::SessionSource;
use crate::repository::Repositories;
use crate::repository::api_token::CreateSessionTokenParams;
use crate::service::auth::backup_codes::BackupCodeManager;
use crate::service::auth::password::{
    dummy_password_hash, hash_password_async, needs_rehash, verify_password_async,
};
use crate::service::auth::s2fa::{S2FA_TTL_SECONDS, generate_s2fa_token};
use crate::service::auth::token::generate_api_token;
use crate::service::auth::totp::TotpManager;
use base::error::AppError;
use infra::rate_limit::{GenericRateLimiter, LoginKeys, LoginRateLimiter};

/// How long a half-authenticated token lives between the password step and the
/// TOTP step of a Web sign-in.
const PENDING_TOKEN_TTL_SECONDS: i64 = 300;

/// A browser session that did not ask to be remembered lasts one day.
const UNREMEMBERED_WEB_SESSION_SECONDS: u64 = 86400;

/// Issuer name the Web UI's authenticator setup registered. Verification only
/// uses the secret, but keeping the same label avoids a second, divergent TOTP
/// object for the same enrollment.
const WEB_TOTP_ISSUER: &str = "Nanofile";

fn non_empty(value: &Option<String>) -> Option<String> {
    value
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

/// Represents all possible outcomes of a login attempt.
pub enum LoginResult {
    /// Login succeeded. Includes the API token and an optional S2FA device trust token.
    Success {
        /// The account the token belongs to, so a caller need not re-resolve the
        /// submitted identifier (which may differ from the stored one in case).
        user_id: i32,
        api_token: String,
        s2fa_token: Option<String>,
        /// Whether `(platform, device_id)` had never signed in before, decided
        /// before the new session row exists. The handler turns it into a
        /// notification, keeping the service layer free of any mail dependency.
        new_device: bool,
    },
    RateLimited,
    BadCredentials,
    AccountDisabled,
    /// The password was correct, but an open registration has not completed its
    /// email verification. No token may be issued.
    EmailUnverified,
    TwoFactorRequired,
    TwoFactorInvalid,
}

/// Why a `seahub-session-pending` token cannot be exchanged for a session.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PendingTokenFailure {
    /// No row for the token, or the row is a full session: a live credential in
    /// the pending cookie is no evidence that the password step happened. The
    /// row is left alone — only a half-authenticated token may be consumed here.
    Unknown,
    /// The five-minute window has passed; the row is deleted.
    Expired,
}

/// The account's second-factor state on the TOTP step.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SecondFactorState {
    /// The 2FA configuration was removed between the password step and now.
    NotConfigured,
    /// 2FA was switched off in between, so signing in again simply passes.
    Disabled,
}

/// Outcome of one step of a Web sign-in.
#[derive(Debug)]
pub enum WebLogin {
    /// A full browser session was created. `max_age_seconds` is the TTL shared by
    /// the session and CSRF cookies (`None` = browser-session cookie: the token
    /// never expires, so no `Max-Age` may be written).
    Session {
        user_id: i32,
        token: String,
        max_age_seconds: Option<u64>,
    },
    /// The password was right and 2FA is on: the caller sets the pending cookie
    /// and redirects to the TOTP page.
    TwoFactorPending {
        pending_token: String,
    },
    RateLimited,
    BadCredentials,
    AccountDisabled,
    /// The password was correct, but the address has not been verified. The
    /// caller renders the resend/verification page for this account.
    EmailUnverified {
        user: Box<infra::entity::user::Model>,
    },
    /// The submitted TOTP or backup code did not verify; the pending token is
    /// kept so the visitor can try again.
    InvalidCode,
    /// The second factor can no longer be completed; the pending token is gone.
    SecondFactorUnavailable(SecondFactorState),
    /// The pending token cannot be used; it has been deleted when it expired.
    PendingExpired(PendingTokenFailure),
}

/// Service handling login authentication, rate limiting and two-factor flows.
pub struct LoginService {
    repos: Arc<Repositories>,
    password_hash_iterations: u32,
    api_token_ttl_days: u64,
    login_rate_limiter: Arc<LoginRateLimiter>,
    /// Failed TOTP/backup-code checks, keyed by `totp:{user}:{client_ip}`. Shared
    /// with `AppState::auth_limiters.totp`, so the Web sign-in and the 2FA setup
    /// page count against the same budget.
    totp_rate_limiter: Arc<GenericRateLimiter>,
}

impl LoginService {
    pub fn new(
        repos: Arc<Repositories>,
        password_hash_iterations: u32,
        api_token_ttl_days: u64,
        login_rate_limiter: Arc<LoginRateLimiter>,
        totp_rate_limiter: Arc<GenericRateLimiter>,
    ) -> Self {
        Self {
            repos,
            password_hash_iterations,
            api_token_ttl_days,
            login_rate_limiter,
            totp_rate_limiter,
        }
    }

    /// Attempt to authenticate a user with the given credentials.
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
        // Client address, (address, account) pair, and the distinct-account spray
        // counter. Deliberately NO username-only key: that would let anyone lock a
        // victim out of their own account from an unrelated address.
        let login_keys = LoginKeys::new(client_ip, username);

        // Check the budget and reserve an in-flight attempt in one atomic step.
        // Without the reservation, a concurrent burst would all pass the check
        // before the first failure is recorded (PBKDF2 takes long enough), and
        // each of them would get a password guess.
        let Some(mut attempt) = self.login_rate_limiter.begin_login_attempt(&login_keys) else {
            return Ok(LoginResult::RateLimited);
        };

        let user_record = match self.verify_credentials(username, password).await? {
            Some(u) => u,
            None => {
                attempt.fail();
                return Ok(LoginResult::BadCredentials);
            }
        };

        if !user_record.is_active {
            attempt.fail();
            return Ok(LoginResult::AccountDisabled);
        }

        if !user_record.email_verified {
            // The password was right, so this is not a failed guess; do not count
            // it toward the lockout. The API path does not mint a link (the web
            // sign-in does), but it must still refuse to issue a token.
            attempt.succeed();
            return Ok(LoginResult::EmailUnverified);
        }

        let mut skip_2fa = false;
        let mut issued_s2fa_token: Option<String> = None;

        let two_fa = self.repos.user_2fa.find_by_user_id(user_record.id).await?;

        if let Some(tfa) = two_fa
            && tfa.enabled
        {
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

                        // Verify and consume the time step, so a code observed
                        // by someone else cannot be replayed inside the ±1-step
                        // skew window.
                        if !TotpManager::verify_and_consume(
                            &self.repos,
                            user_record.id,
                            &totp,
                            otp_code,
                        )
                        .await
                        {
                            attempt.fail();
                            return Ok(LoginResult::TwoFactorInvalid);
                        }

                        if trust_device {
                            use crate::repository::s2fa_token::CreateS2faTokenParams;
                            let s2fa_token_value = generate_s2fa_token();
                            let now = chrono::Utc::now().timestamp();
                            // One trust per device: the previous row is superseded
                            // by the one about to be issued, and keeping it would
                            // inflate the trusted-device count.
                            if let Some(id) = device_id.as_deref().filter(|id| !id.is_empty()) {
                                self.repos
                                    .s2fa_token
                                    .delete_by_user_and_device(user_record.id, id)
                                    .await?;
                            }
                            self.repos
                                .s2fa_token
                                .create_s2fa_token(CreateS2faTokenParams {
                                    user_id: user_record.id,
                                    token: s2fa_token_value.clone(),
                                    device_id: device_id.clone(),
                                    device_name: device_name.clone(),
                                    created_at: now,
                                    expires_at: now + S2FA_TTL_SECONDS,
                                })
                                .await?;
                            issued_s2fa_token = Some(s2fa_token_value);
                        }
                    }
                    None => {
                        return Ok(LoginResult::TwoFactorRequired);
                    }
                }
            }
        }

        // The password (and second factor, when one was required) was correct:
        // release the reservation and forgive the address/pair counters, exactly
        // as the pre-reservation code did. The distinct-account spray history is
        // intentionally kept.
        attempt.succeed();

        // Upgrade the stored hash if its cost predates the configured one.
        self.rehash_password_if_needed(&user_record, password).await;

        // Ask before inserting: the session about to be created is the new row,
        // and counting it would make every login look familiar.
        let new_device = match (non_empty(&platform), non_empty(&device_id)) {
            (Some(platform), Some(device_id)) => {
                !self
                    .repos
                    .api_token
                    .has_session_for_device(user_record.id, &platform, &device_id)
                    .await?
            }
            // A client with no device identity cannot be compared with anything,
            // so it is never announced as new.
            _ => false,
        };

        let token_value = generate_api_token();
        let now = chrono::Utc::now().timestamp();

        self.repos
            .api_token
            .create_session_token(crate::repository::api_token::CreateSessionTokenParams {
                user_id: user_record.id,
                token: token_value.clone(),
                created_at: now,
                expires_at: if self.api_token_ttl_days > 0 {
                    Some(now + self.api_token_ttl_days as i64 * 86400)
                } else {
                    None
                },
                device_id,
                platform,
                device_name,
                client_version,
                is_pending: false,
                // A client application, not a browser, holds this token; its
                // `User-Agent` is the client library's, so the device fields identify it.
                source: SessionSource::Client,
                user_agent: None,
            })
            .await?;

        self.repos
            .user
            .touch_last_login(user_record.id, now)
            .await?;

        Ok(LoginResult::Success {
            user_id: user_record.id,
            api_token: token_value,
            s2fa_token: issued_s2fa_token,
            new_device,
        })
    }

    /// First step of the Web UI sign-in: check the password, then either finish
    /// the session or hand out a pending 2FA token.
    ///
    /// The handler keeps only the HTTP mechanics (cookies, redirects, rendering,
    /// CSRF, proof of work, mail); the security decisions — rate limiting, the
    /// enumeration-safe password check, rehash, 2FA branching — live here so the
    /// Web and API paths cannot drift apart.
    pub async fn begin_web_login(
        &self,
        username: &str,
        password: &str,
        client_ip: &str,
        user_agent: Option<String>,
        remember_me: bool,
    ) -> Result<WebLogin, AppError> {
        let login_keys = LoginKeys::new(client_ip, username);
        let Some(mut attempt) = self.login_rate_limiter.begin_login_attempt(&login_keys) else {
            return Ok(WebLogin::RateLimited);
        };

        let Some(user_record) = self.verify_credentials(username, password).await? else {
            attempt.fail();
            return Ok(WebLogin::BadCredentials);
        };

        if !user_record.is_active {
            attempt.fail();
            return Ok(WebLogin::AccountDisabled);
        }

        // The password was right, so this is not a failed guess: release the
        // reservation and forgive the address/pair counters, exactly as the API
        // path does. A correct password on an unverified account must not count
        // toward the lockout either.
        attempt.succeed();

        if !user_record.email_verified {
            return Ok(WebLogin::EmailUnverified {
                user: Box::new(user_record),
            });
        }

        self.rehash_password_if_needed(&user_record, password).await;

        let two_fa = self.repos.user_2fa.find_by_user_id(user_record.id).await?;
        if let Some(tfa) = two_fa
            && tfa.enabled
        {
            let pending_token = self.create_pending_token(user_record.id).await?;
            return Ok(WebLogin::TwoFactorPending { pending_token });
        }

        let now = chrono::Utc::now().timestamp();
        let token = generate_api_token();
        // "Remember me" keeps the configured lifetime; unchecked gives a
        // one-day session. A configured lifetime of 0 means no expiry, which a
        // session cookie (no Max-Age) expresses.
        let max_age_seconds = if remember_me {
            self.remembered_web_session_seconds()
        } else {
            Some(UNREMEMBERED_WEB_SESSION_SECONDS)
        };
        self.create_web_session(
            user_record.id,
            &token,
            now,
            max_age_seconds.map(|seconds| now + seconds as i64),
            user_agent,
        )
        .await?;
        self.repos
            .user
            .touch_last_login(user_record.id, now)
            .await?;

        Ok(WebLogin::Session {
            user_id: user_record.id,
            token,
            max_age_seconds,
        })
    }

    /// Second step of the Web UI sign-in: exchange a pending token plus a TOTP or
    /// backup code for a full browser session.
    pub async fn complete_web_two_factor(
        &self,
        pending_token: &str,
        otp: &str,
        client_ip: &str,
        user_agent: Option<String>,
    ) -> Result<WebLogin, AppError> {
        let Some(record) = self.repos.api_token.find_by_token(pending_token).await? else {
            return Ok(WebLogin::PendingExpired(PendingTokenFailure::Unknown));
        };

        if let Some(expires_at) = record.expires_at
            && chrono::Utc::now().timestamp() > expires_at
        {
            let _ = self.repos.api_token.delete_by_token(pending_token).await;
            return Ok(WebLogin::PendingExpired(PendingTokenFailure::Expired));
        }

        // Only a 2FA-pending token may be presented here. Without this check any
        // live account token placed in the `seahub-session-pending` cookie is
        // accepted as evidence that the password step already happened, which is
        // the precondition this page exists to establish (every other consumer of
        // an API token rejects `is_pending`).
        if !record.is_pending {
            return Ok(WebLogin::PendingExpired(PendingTokenFailure::Unknown));
        }

        let user_id = record.user_id;
        let totp_key = format!("totp:{user_id}:{client_ip}");
        if self.totp_rate_limiter.is_limited(&totp_key) {
            // Force a new sign-in: the code page must not stay reachable once its
            // budget is spent.
            let _ = self.repos.api_token.delete_by_token(pending_token).await;
            return Ok(WebLogin::RateLimited);
        }

        let Some(two_fa) = self.repos.user_2fa.find_by_user_id(user_id).await? else {
            let _ = self.repos.api_token.delete_by_token(pending_token).await;
            return Ok(WebLogin::SecondFactorUnavailable(
                SecondFactorState::NotConfigured,
            ));
        };
        if !two_fa.enabled {
            // 2FA was disabled since the pending token was created — signing in
            // again simply passes without a second factor.
            let _ = self.repos.api_token.delete_by_token(pending_token).await;
            return Ok(WebLogin::SecondFactorUnavailable(
                SecondFactorState::Disabled,
            ));
        }

        let user_record = self
            .repos
            .user
            .find_by_id(user_id)
            .await?
            .ok_or(AppError::Unauthorized)?;

        let totp =
            TotpManager::create_totp(&two_fa.totp_secret, &user_record.email, WEB_TOTP_ISSUER)
                .map_err(|e| AppError::internal(e.to_string()))?;

        // Verify and consume the time step, so a code observed by someone else
        // cannot be replayed inside the ±1-step skew window. Backup codes are the
        // fallback when the authenticator is lost.
        let code_valid = TotpManager::verify_and_consume(&self.repos, user_id, &totp, otp).await;
        let backup_valid = if code_valid {
            false
        } else {
            BackupCodeManager::verify_code(&self.repos, user_id, otp)
                .await
                .unwrap_or(false)
        };

        if !code_valid && !backup_valid {
            self.totp_rate_limiter.record_attempt(&totp_key);
            // Keep the pending token: the visitor may retry within its lifetime.
            return Ok(WebLogin::InvalidCode);
        }

        self.totp_rate_limiter.clear(&totp_key);
        let _ = self.repos.api_token.delete_by_token(pending_token).await;

        let now = chrono::Utc::now().timestamp();
        let token = generate_api_token();
        // The TOTP step makes this a full sign-in whatever "remember me" said:
        // the pending token does not carry the checkbox, so the configured
        // lifetime applies (this is the existing 2FA success path).
        let max_age_seconds = Some(self.api_token_ttl_days.saturating_mul(86400));
        self.create_web_session(
            user_id,
            &token,
            now,
            Some(now + (self.api_token_ttl_days as i64 * 86400)),
            user_agent,
        )
        .await?;
        self.repos.user.touch_last_login(user_id, now).await?;

        Ok(WebLogin::Session {
            user_id,
            token,
            max_age_seconds,
        })
    }

    /// Look up the account and check the password.
    ///
    /// `Ok(None)` covers both an unknown address and a wrong password. A missing
    /// address still spends one PBKDF2 run against a cost-matched dummy hash, so
    /// the two cases take the same time (username enumeration by response time).
    async fn verify_credentials(
        &self,
        username: &str,
        password: &str,
    ) -> Result<Option<infra::entity::user::Model>, AppError> {
        let Some(user_record) = self.repos.user.find_by_email(username).await? else {
            let _ = verify_password_async(
                password.to_string(),
                dummy_password_hash(self.password_hash_iterations),
                self.password_hash_iterations,
            )
            .await;
            return Ok(None);
        };

        if !verify_password_async(
            password.to_string(),
            user_record.password_hash.clone(),
            self.password_hash_iterations,
        )
        .await
        {
            return Ok(None);
        }

        Ok(Some(user_record))
    }

    /// Upgrade a stored hash if its cost predates the configured one.
    /// Best-effort: a failed write must not block a correct password.
    async fn rehash_password_if_needed(
        &self,
        user_record: &infra::entity::user::Model,
        password: &str,
    ) {
        if !needs_rehash(&user_record.password_hash, self.password_hash_iterations) {
            return;
        }
        let upgraded =
            hash_password_async(password.to_string(), self.password_hash_iterations).await;
        if let Err(e) = self
            .repos
            .user
            .update_password(user_record.id, upgraded)
            .await
        {
            tracing::warn!(
                "failed to upgrade password hash for user {}: {e}",
                user_record.id
            );
        }
    }

    /// Lifetime of a remembered browser session, or `None` when the setting is 0
    /// (no expiry — the cookie is then a browser-session cookie).
    fn remembered_web_session_seconds(&self) -> Option<u64> {
        (self.api_token_ttl_days > 0).then(|| self.api_token_ttl_days.saturating_mul(86400))
    }

    /// Create a `SessionSource::Web` session row.
    async fn create_web_session(
        &self,
        user_id: i32,
        token: &str,
        now: i64,
        expires_at: Option<i64>,
        user_agent: Option<String>,
    ) -> Result<(), AppError> {
        self.repos
            .api_token
            .create_session_token(CreateSessionTokenParams {
                user_id,
                token: token.to_string(),
                created_at: now,
                expires_at,
                device_id: None,
                platform: None,
                device_name: None,
                client_version: None,
                is_pending: false,
                source: SessionSource::Web,
                user_agent,
            })
            .await
            .map_err(|e| AppError::internal(format!("failed to create session token: {e}")))?;
        Ok(())
    }

    /// Mint the short-lived half-credential that the TOTP page exchanges for a
    /// session. It carries no device identity and no `User-Agent`: it is never
    /// listed and lives for five minutes.
    async fn create_pending_token(&self, user_id: i32) -> Result<String, AppError> {
        let token = generate_api_token();
        let now = chrono::Utc::now().timestamp();
        self.repos
            .api_token
            .create_session_token(CreateSessionTokenParams {
                user_id,
                token: token.clone(),
                created_at: now,
                expires_at: Some(now + PENDING_TOKEN_TTL_SECONDS),
                device_id: None,
                platform: None,
                device_name: None,
                client_version: None,
                is_pending: true,
                source: SessionSource::Web,
                // Never listed; a user agent here would only park a
                // secret-adjacent value.
                user_agent: None,
            })
            .await
            .map_err(|e| AppError::internal(format!("failed to create pending token: {e}")))?;
        Ok(token)
    }

    /// Invalidate a device session by deleting the API token.
    pub async fn logout_device(&self, token_str: &str) -> Result<(), AppError> {
        self.repos.api_token.delete_by_token(token_str).await?;
        Ok(())
    }
}
