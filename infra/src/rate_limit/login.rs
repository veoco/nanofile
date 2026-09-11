/// In-memory login attempt rate limiter.
///
/// Tracks failed login attempts by key (IP or "ip:username") and
/// prevents further attempts after a configurable threshold within
/// a configurable time window. A third counter tracks how many *distinct*
/// accounts one address has failed on, so credential spraying is caught even
/// when a successful login keeps resetting the per-address counter.
use std::collections::HashMap;
use std::sync::{Mutex, MutexGuard, PoisonError};
use std::time::{SystemTime, UNIX_EPOCH};

/// Throttle keys for a single login attempt.
///
/// The pair key deliberately includes the client address: a username-only
/// counter lets anyone lock a victim out of their own account from an
/// unrelated address, and the victim cannot clear it (only a *successful*
/// login clears a key).
#[derive(Debug, Clone)]
pub struct LoginKeys {
    client_ip: String,
    username: String,
    ip_key: String,
    pair_key: String,
}

impl LoginKeys {
    pub fn new(client_ip: &str, username: &str) -> Self {
        Self {
            client_ip: client_ip.to_string(),
            username: username.to_string(),
            ip_key: format!("login:ip:{client_ip}"),
            pair_key: format!("login:pair:{client_ip}:{username}"),
        }
    }
}

/// Upper bound on the number of client addresses the spray detector tracks, so
/// a flood of distinct (real) source addresses cannot grow the map forever.
/// Once full, the address with the oldest failure is evicted.
const MAX_SPRAY_IPS: usize = 4096;

pub struct LoginRateLimiter {
    attempts: Mutex<HashMap<String, Vec<i64>>>,
    /// Distinct account names that failed from each address within the window
    /// (`ip -> account -> last failure`). A successful login does NOT clear
    /// this: see [`LoginRateLimiter::clear_login_failure`].
    spray: Mutex<HashMap<String, HashMap<String, i64>>>,
    max_attempts: u32,
    lockout_secs: i64,
    /// Max distinct accounts one address may fail on before it is blocked
    /// (0 = disabled).
    max_distinct_usernames: u32,
}

impl LoginRateLimiter {
    pub fn new(max_attempts: u32, lockout_secs: u64, max_distinct_usernames: u32) -> Self {
        Self {
            attempts: Mutex::new(HashMap::new()),
            spray: Mutex::new(HashMap::new()),
            max_attempts,
            lockout_secs: lockout_secs as i64,
            max_distinct_usernames,
        }
    }

    fn now() -> i64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64
    }

    /// Acquire the internal mutex, recovering from a poisoned state.
    /// The rate limiter's HashMap has no invariants that would be violated
    /// by a panic in another thread, so poison recovery is safe.
    fn lock(&self) -> MutexGuard<'_, HashMap<String, Vec<i64>>> {
        self.attempts.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Record a failed login attempt for the given key.
    pub fn record_failure(&self, key: &str) {
        let now = Self::now();
        let mut map = self.lock();
        let timestamps = map.entry(key.to_string()).or_default();
        timestamps.push(now);
        // Trim entries older than the lockout window to bound memory.
        let cutoff = now - self.lockout_secs;
        timestamps.retain(|&t| t > cutoff);
    }

    /// Check if the given key is currently locked out.
    pub fn is_locked(&self, key: &str) -> bool {
        let now = Self::now();
        let cutoff = now - self.lockout_secs;
        let mut map = self.lock();
        let (stale, limited) = match map.get_mut(key) {
            Some(timestamps) => {
                timestamps.retain(|&t| t > cutoff);
                (
                    timestamps.is_empty(),
                    timestamps.len() as u32 >= self.max_attempts,
                )
            }
            None => (false, false),
        };
        if stale {
            // All attempts expired — drop the key so the map doesn't grow
            // without bound on continuously-failing keys.
            map.remove(key);
            false
        } else {
            limited
        }
    }

    /// Check whether any of the given keys is currently locked out, using a
    /// single lock acquisition for all keys.
    pub fn is_any_locked(&self, keys: &[&str]) -> bool {
        let now = Self::now();
        let cutoff = now - self.lockout_secs;
        let mut map = self.lock();
        for key in keys {
            let (stale, limited) = match map.get_mut(*key) {
                Some(timestamps) => {
                    timestamps.retain(|&t| t > cutoff);
                    (
                        timestamps.is_empty(),
                        timestamps.len() as u32 >= self.max_attempts,
                    )
                }
                None => (false, false),
            };
            if stale {
                map.remove(*key);
            } else if limited {
                return true;
            }
        }
        false
    }

    /// Record failed attempts for several keys in a single lock acquisition.
    pub fn record_failures(&self, keys: &[&str]) {
        let now = Self::now();
        let cutoff = now - self.lockout_secs;
        let mut map = self.lock();
        for key in keys {
            let timestamps = map.entry((*key).to_string()).or_default();
            timestamps.push(now);
            timestamps.retain(|&t| t > cutoff);
        }
    }

    /// Clear all recorded attempts for a key (called on successful login).
    pub fn clear(&self, key: &str) {
        let mut map = self.lock();
        map.remove(key);
    }

    /// Whether this attempt must be rejected before any credential work.
    ///
    /// Three limits are consulted: failures from the address, failures for this
    /// (address, account) pair, and failures spread across too many distinct
    /// accounts from the address.
    pub fn is_login_blocked(&self, keys: &LoginKeys) -> bool {
        self.is_any_locked(&[keys.ip_key.as_str(), keys.pair_key.as_str()])
            || self.is_spraying(&keys.client_ip)
    }

    /// Record a failed login attempt against all three limits.
    pub fn record_login_failure(&self, keys: &LoginKeys) {
        self.record_failures(&[keys.ip_key.as_str(), keys.pair_key.as_str()]);
        self.record_username_failure(&keys.client_ip, &keys.username);
    }

    /// Record a successful login.
    ///
    /// The address and pair counters are forgiven — otherwise one user
    /// fat-fingering their password behind a shared NAT address would lock out
    /// everyone behind it. The distinct-account history is deliberately kept:
    /// an attacker holding one valid account must not be able to reset their
    /// spray budget by logging into it after every few guesses.
    pub fn clear_login_failure(&self, keys: &LoginKeys) {
        self.clear(&keys.ip_key);
        self.clear(&keys.pair_key);
    }

    /// Remember that `username` failed from `ip`, for the spray detector.
    fn record_username_failure(&self, ip: &str, username: &str) {
        let now = Self::now();
        let cutoff = now - self.lockout_secs;
        let mut map = self.spray.lock().unwrap_or_else(PoisonError::into_inner);
        if !map.contains_key(ip) && map.len() >= MAX_SPRAY_IPS {
            // Evict the address whose most recent failure is oldest. Failure
            // timestamps are refreshed on every attempt, so this tracks the
            // least recently active source.
            let oldest = map
                .iter()
                .min_by_key(|(_, per_ip)| per_ip.values().copied().max().unwrap_or(i64::MIN))
                .map(|(addr, _)| addr.clone());
            if let Some(addr) = oldest {
                map.remove(&addr);
            }
        }
        let per_ip = map.entry(ip.to_string()).or_default();
        per_ip.retain(|_, t| *t > cutoff);
        per_ip.insert(username.to_string(), now);
    }

    /// Whether `ip` has failed on at least the configured number of distinct
    /// accounts within the window.
    fn is_spraying(&self, ip: &str) -> bool {
        if self.max_distinct_usernames == 0 {
            return false;
        }
        let now = Self::now();
        let cutoff = now - self.lockout_secs;
        let mut map = self.spray.lock().unwrap_or_else(PoisonError::into_inner);
        let (empty, distinct) = match map.get_mut(ip) {
            Some(per_ip) => {
                per_ip.retain(|_, t| *t > cutoff);
                (per_ip.is_empty(), per_ip.len() as u32)
            }
            None => return false,
        };
        if empty {
            map.remove(ip);
            return false;
        }
        distinct >= self.max_distinct_usernames
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_allows_first_attempt() {
        let limiter = LoginRateLimiter::new(3, 60, 0);
        assert!(!limiter.is_locked("test-user"));
    }

    #[test]
    fn test_locks_after_threshold() {
        let limiter = LoginRateLimiter::new(3, 60, 0);
        limiter.record_failure("test-user");
        limiter.record_failure("test-user");
        limiter.record_failure("test-user");
        assert!(limiter.is_locked("test-user"));
    }

    #[test]
    fn test_clear_resets() {
        let limiter = LoginRateLimiter::new(3, 60, 0);
        limiter.record_failure("test-user");
        limiter.record_failure("test-user");
        limiter.record_failure("test-user");
        assert!(limiter.is_locked("test-user"));
        limiter.clear("test-user");
        assert!(!limiter.is_locked("test-user"));
    }

    #[test]
    fn test_allows_below_threshold() {
        let limiter = LoginRateLimiter::new(5, 60, 0);
        limiter.record_failure("test-user");
        limiter.record_failure("test-user");
        assert!(!limiter.is_locked("test-user"));
    }

    #[test]
    fn test_is_any_locked() {
        let limiter = LoginRateLimiter::new(3, 60, 0);
        limiter.record_failure("key-a");
        limiter.record_failure("key-a");
        limiter.record_failure("key-a");
        assert!(limiter.is_any_locked(&["key-a", "key-b"]));
        assert!(!limiter.is_any_locked(&["key-b"]));
    }

    #[test]
    fn test_record_failures_batch() {
        let limiter = LoginRateLimiter::new(3, 60, 0);
        limiter.record_failures(&["key-a", "key-b"]);
        limiter.record_failures(&["key-a", "key-b"]);
        assert!(!limiter.is_locked("key-a"));
        limiter.record_failures(&["key-a", "key-b"]);
        assert!(limiter.is_any_locked(&["key-a"]));
    }

    #[test]
    fn test_expired_key_is_pruned() {
        let limiter = LoginRateLimiter::new(3, 60, 0);
        let old = LoginRateLimiter::now() - 120;
        limiter
            .attempts
            .lock()
            .unwrap()
            .insert("stale".to_string(), vec![old]);
        assert!(!limiter.is_locked("stale"));
        assert!(!limiter.attempts.lock().unwrap().contains_key("stale"));
    }

    /// An attacker must not be able to lock a victim out of their own account:
    /// failures from one address only throttle that address for that account.
    #[test]
    fn pair_key_scopes_failures_to_the_source_address() {
        let limiter = LoginRateLimiter::new(3, 60, 0);
        let attacker = LoginKeys::new("203.0.113.9", "victim@example.com");
        for _ in 0..3 {
            assert!(!limiter.is_login_blocked(&attacker));
            limiter.record_login_failure(&attacker);
        }
        assert!(limiter.is_login_blocked(&attacker));

        // The victim's own address is unaffected and can still authenticate.
        let victim = LoginKeys::new("198.51.100.4", "victim@example.com");
        assert!(!limiter.is_login_blocked(&victim));
        limiter.clear_login_failure(&victim);
        assert!(!limiter.is_login_blocked(&victim));

        // A third address is unaffected by the attacker's failures.
        assert!(!limiter.is_login_blocked(&LoginKeys::new("192.0.2.7", "victim@example.com")));

        // The per-address counter is the credential-stuffing control, so the
        // attacker's address is blocked for other accounts too.
        let other = LoginKeys::new("203.0.113.9", "someone-else@example.com");
        assert!(limiter.is_login_blocked(&other));
    }

    /// A successful login forgives the address and pair counters, so a shared
    /// NAT address is not locked out by one user's typos.
    #[test]
    fn success_clears_address_and_pair_counters() {
        let limiter = LoginRateLimiter::new(3, 60, 2);
        let keys = LoginKeys::new("198.51.100.4", "user@example.com");
        for _ in 0..2 {
            limiter.record_login_failure(&keys);
        }
        assert!(!limiter.is_login_blocked(&keys));
        limiter.record_login_failure(&keys);
        assert!(limiter.is_login_blocked(&keys));

        limiter.clear_login_failure(&keys);
        assert!(!limiter.is_login_blocked(&keys));
    }

    /// Spraying distinct accounts stays blocked across successful logins: one
    /// known-good credential must not reset the attacker's budget.
    #[test]
    fn success_does_not_reset_distinct_account_spraying() {
        let limiter = LoginRateLimiter::new(5, 60, 3);
        let good = LoginKeys::new("203.0.113.9", "attacker@example.com");

        for name in ["a@example.com", "b@example.com", "c@example.com"] {
            let keys = LoginKeys::new("203.0.113.9", name);
            assert!(!limiter.is_login_blocked(&keys), "{name} should be tried");
            limiter.record_login_failure(&keys);
        }

        // The attacker's own valid account is now blocked too, because the
        // address has been seen failing on three distinct accounts and a
        // success clears only its own counters.
        limiter.clear_login_failure(&good);
        assert!(limiter.is_login_blocked(&good));
        assert!(limiter.is_login_blocked(&LoginKeys::new("203.0.113.9", "d@example.com")));

        // A different address is unaffected.
        assert!(!limiter.is_login_blocked(&LoginKeys::new("198.51.100.4", "a@example.com")));
    }

    /// `max_distinct_usernames == 0` disables the spray detector.
    #[test]
    fn spray_detection_can_be_disabled() {
        let limiter = LoginRateLimiter::new(5, 60, 0);
        for name in ["a@example.com", "b@example.com", "c@example.com"] {
            limiter.record_login_failure(&LoginKeys::new("203.0.113.9", name));
        }
        assert!(!limiter.is_login_blocked(&LoginKeys::new("203.0.113.9", "d@example.com")));
    }

    /// An address that stops failing is forgotten once its window expires.
    #[test]
    fn spray_history_expires_with_the_window() {
        let limiter = LoginRateLimiter::new(5, 60, 2);
        limiter.record_login_failure(&LoginKeys::new("203.0.113.9", "a@example.com"));
        limiter.record_login_failure(&LoginKeys::new("203.0.113.9", "b@example.com"));
        assert!(limiter.is_login_blocked(&LoginKeys::new("203.0.113.9", "c@example.com")));

        // Age the recorded failures past the window.
        let old = LoginRateLimiter::now() - 120;
        {
            let mut map = limiter.spray.lock().unwrap();
            if let Some(per_ip) = map.get_mut("203.0.113.9") {
                for ts in per_ip.values_mut() {
                    *ts = old;
                }
            }
        }
        assert!(!limiter.is_login_blocked(&LoginKeys::new("203.0.113.9", "c@example.com")));
        assert!(!limiter.spray.lock().unwrap().contains_key("203.0.113.9"));
    }
}
