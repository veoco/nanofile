use std::sync::Arc;

use infra::config::AuthConfig;
use infra::rate_limit::{GenericRateLimiter, LoginRateLimiter};

/// Aggregated authentication rate limiters shared via `AppState`.
///
/// Collapses the five independent limiter fields that used to live on
/// `AppState` into one cohesive unit, since they are all constructed from
/// the same `AuthConfig` and used only by auth paths.
pub struct AuthRateLimiters {
    pub login: Arc<LoginRateLimiter>,
    pub password_reset: Arc<GenericRateLimiter>,
    pub registration: Arc<GenericRateLimiter>,
    pub totp: Arc<GenericRateLimiter>,
    pub disable_2fa: Arc<GenericRateLimiter>,
}

impl AuthRateLimiters {
    pub fn new(cfg: &AuthConfig) -> Arc<Self> {
        Arc::new(Self {
            login: Arc::new(LoginRateLimiter::new(
                cfg.max_login_attempts,
                cfg.lockout_duration_secs,
            )),
            password_reset: Arc::new(GenericRateLimiter::new(
                cfg.password_reset_max_per_hour.max(1),
                3600,
            )),
            registration: Arc::new(GenericRateLimiter::new(
                cfg.registration_max_per_hour.max(1),
                3600,
            )),
            totp: Arc::new(GenericRateLimiter::new(cfg.totp_max_attempts.max(1), 300)),
            disable_2fa: Arc::new(GenericRateLimiter::new(cfg.totp_max_attempts.max(1), 300)),
        })
    }
}
