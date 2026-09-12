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
    pub link_password: Arc<GenericRateLimiter>,
    /// Failed encrypted-library password checks, keyed by `(user, repo)`.
    /// The wire protocol fixes the KDF iteration count, so throttling is the
    /// only available control against offline-fast guessing online.
    pub repo_password: Arc<GenericRateLimiter>,
    pub share_download: Arc<GenericRateLimiter>,
    /// Failed WebDAV Basic-auth attempts, keyed by client IP. Successful
    /// requests are never counted, so a working client cannot trip this.
    pub webdav_auth: Arc<GenericRateLimiter>,
    pub reindex: Arc<GenericRateLimiter>,
    pub search: Arc<GenericRateLimiter>,
}

impl AuthRateLimiters {
    pub fn new(cfg: &AuthConfig) -> Arc<Self> {
        Arc::new(Self {
            login: Arc::new(LoginRateLimiter::new(
                cfg.max_login_attempts,
                cfg.lockout_duration_secs,
                cfg.max_distinct_usernames_per_ip,
            )),
            password_reset: Arc::new(GenericRateLimiter::new(
                cfg.password_reset_max_per_hour,
                3600,
            )),
            registration: Arc::new(GenericRateLimiter::new(cfg.registration_max_per_hour, 3600)),
            totp: Arc::new(GenericRateLimiter::new(cfg.totp_max_attempts, 300)),
            disable_2fa: Arc::new(GenericRateLimiter::new(cfg.totp_max_attempts, 300)),
            link_password: Arc::new(GenericRateLimiter::new(
                cfg.link_password_max_per_hour,
                3600,
            )),
            repo_password: Arc::new(GenericRateLimiter::new(
                cfg.repo_password_max_per_hour,
                3600,
            )),
            share_download: Arc::new(GenericRateLimiter::new(
                cfg.share_download_max_per_minute,
                60,
            )),
            webdav_auth: Arc::new(GenericRateLimiter::new(
                cfg.webdav_max_failures_per_5min,
                300,
            )),
            reindex: Arc::new(GenericRateLimiter::new(cfg.reindex_max_per_hour, 3600)),
            search: Arc::new(GenericRateLimiter::new(cfg.search_max_per_minute, 60)),
        })
    }
}

impl AuthRateLimiters {
    /// Rate-limit key for failed encrypted-library password checks, scoped to
    /// one (user, repo) pair.
    ///
    /// **Every** endpoint that verifies a library password or magic must use
    /// this key. The library KDF iteration count is fixed at 1000 by the
    /// Seafile wire protocol, so this limiter is the only control against
    /// online guessing; a second endpoint using a different key would hand the
    /// attacker an unmetered oracle.
    pub fn repo_password_key(user_id: i32, repo_id: &str) -> String {
        format!("repo_pw:{user_id}:{repo_id}")
    }

    /// Whether this (user, repo) pair has exhausted its failed-attempt budget.
    pub fn is_repo_password_limited(&self, user_id: i32, repo_id: &str) -> bool {
        self.repo_password
            .is_limited(&Self::repo_password_key(user_id, repo_id))
    }

    /// Count one failed library password/magic guess.
    pub fn record_repo_password_failure(&self, user_id: i32, repo_id: &str) {
        self.repo_password
            .record_attempt(&Self::repo_password_key(user_id, repo_id));
    }

    /// Clear the failure budget after a successful verification.
    ///
    /// Only failures may ever be counted: the Android client re-submits its own
    /// cached password before every download, so counting successes would lock
    /// a legitimate user out of their own library.
    pub fn clear_repo_password_failures(&self, user_id: i32, repo_id: &str) {
        self.repo_password
            .clear(&Self::repo_password_key(user_id, repo_id));
    }
}

#[cfg(test)]
mod tests {
    use super::AuthRateLimiters;
    use infra::config::AuthConfig;
    use infra::rate_limit::LoginKeys;

    /// Every field documented as "(0 = unlimited)" must really disable its
    /// limiter. The config used to be clamped with `.max(1)` on construction,
    /// which turned "unlimited" into "reject the first request".
    #[test]
    fn zero_config_disables_every_limiter() {
        let cfg = AuthConfig {
            max_login_attempts: 0,
            max_distinct_usernames_per_ip: 0,
            password_reset_max_per_hour: 0,
            registration_max_per_hour: 0,
            totp_max_attempts: 0,
            link_password_max_per_hour: 0,
            repo_password_max_per_hour: 0,
            share_download_max_per_minute: 0,
            webdav_max_failures_per_5min: 0,
            reindex_max_per_hour: 0,
            search_max_per_minute: 0,
            ..Default::default()
        };
        let limiters = AuthRateLimiters::new(&cfg);

        let keys = LoginKeys::new("203.0.113.9", "user@example.com");
        for _ in 0..10 {
            limiters.login.record_login_failure(&keys);
        }
        assert!(
            !limiters.login.is_login_blocked(&keys),
            "0 attempts must disable the login lockout instead of locking everyone out"
        );

        for limiter in [
            &limiters.password_reset,
            &limiters.registration,
            &limiters.totp,
            &limiters.disable_2fa,
            &limiters.link_password,
            &limiters.repo_password,
            &limiters.share_download,
            &limiters.webdav_auth,
            &limiters.reindex,
            &limiters.search,
        ] {
            for _ in 0..10 {
                limiter.record_attempt("k");
            }
            assert!(!limiter.is_limited("k"));
        }
    }

    /// The shipped defaults must still throttle.
    #[test]
    fn default_config_throttles() {
        let cfg = AuthConfig::default();
        let limiters = AuthRateLimiters::new(&cfg);
        for _ in 0..cfg.search_max_per_minute {
            limiters.search.record_attempt("k");
        }
        assert!(limiters.search.is_limited("k"));

        let keys = LoginKeys::new("203.0.113.9", "user@example.com");
        for _ in 0..cfg.max_login_attempts {
            limiters.login.record_login_failure(&keys);
        }
        assert!(limiters.login.is_login_blocked(&keys));
    }
}
