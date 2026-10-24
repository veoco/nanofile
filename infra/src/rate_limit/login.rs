/// In-memory login attempt rate limiter.
///
/// Tracks failures by key (IP or `ip:username`), locks out after a threshold in a
/// window, and counts distinct failed accounts per address so credential spraying is
/// caught even when a success keeps resetting the per-address counter.
use std::collections::HashMap;
use std::sync::atomic::{AtomicI64, AtomicU32, Ordering};
use std::sync::{Mutex, MutexGuard, PoisonError};
use std::time::{SystemTime, UNIX_EPOCH};

/// Throttle keys for one login attempt.
///
/// The pair includes the client address: a username-only counter would let anyone
/// lock a victim out from an unrelated address, and only a successful login clears a
/// key.
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

/// Cap on the client addresses the spray detector tracks, so a flood of distinct
/// addresses cannot grow the map forever; the oldest failure is evicted.
const MAX_SPRAY_IPS: usize = 4096;

/// Cap on the failed-attempt map. Keys combine a client-supplied address and account
/// and one attempt records two keys, so window trimming cannot bound a flood of fresh
/// IPv6 addresses; the oldest keys are evicted instead. Eviction fails open, the right
/// trade-off against unbounded memory.
const MAX_ATTEMPT_KEYS: usize = 50_000;

pub struct LoginRateLimiter {
    attempts: Mutex<HashMap<String, Vec<i64>>>,
    /// `ip -> account -> last failure`; a successful login deliberately does NOT clear
    /// this (see [`LoginRateLimiter::clear_login_failure`]).
    spray: Mutex<HashMap<String, HashMap<String, i64>>>,
    /// Atomic so an administrator can change the budget on a running server; what is
    /// already counted is kept, so raising the limit does not clear lockouts.
    max_attempts: AtomicU32,
    lockout_secs: AtomicI64,
    /// Max distinct accounts one address may fail on before it is blocked
    /// (0 = disabled).
    max_distinct_usernames: AtomicU32,
}

impl LoginRateLimiter {
    pub fn new(max_attempts: u32, lockout_secs: u64, max_distinct_usernames: u32) -> Self {
        Self {
            attempts: Mutex::new(HashMap::new()),
            spray: Mutex::new(HashMap::new()),
            max_attempts: AtomicU32::new(max_attempts),
            lockout_secs: AtomicI64::new(lockout_secs as i64),
            max_distinct_usernames: AtomicU32::new(max_distinct_usernames),
        }
    }

    /// Replace the budgets without touching what is already counted.
    pub fn set_limits(&self, max_attempts: u32, lockout_secs: u64, max_distinct_usernames: u32) {
        self.max_attempts.store(max_attempts, Ordering::Relaxed);
        self.lockout_secs
            .store(lockout_secs as i64, Ordering::Relaxed);
        self.max_distinct_usernames
            .store(max_distinct_usernames, Ordering::Relaxed);
    }

    /// The configured per-key attempt budget.
    fn limit(&self) -> u32 {
        self.max_attempts.load(Ordering::Relaxed)
    }

    /// The configured lockout window, in seconds.
    fn window(&self) -> i64 {
        self.lockout_secs.load(Ordering::Relaxed)
    }

    /// The configured spray-detector budget.
    fn distinct_limit(&self) -> u32 {
        self.max_distinct_usernames.load(Ordering::Relaxed)
    }

    /// Whether the failed-attempt lockout is disabled (`0`).
    fn lockout_disabled(&self) -> bool {
        self.limit() == 0
    }

    fn now() -> i64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64
    }

    /// Acquire the mutex, recovering from poison: the map has no invariant a panic
    /// elsewhere could violate.
    fn lock(&self) -> MutexGuard<'_, HashMap<String, Vec<i64>>> {
        self.attempts.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Evict the least recently active keys once [`MAX_ATTEMPT_KEYS`] is exceeded, so the
    /// common path stays a couple of hash operations.
    fn shrink_attempts(map: &mut HashMap<String, Vec<i64>>) {
        super::evict_oldest(map, MAX_ATTEMPT_KEYS);
    }

    /// Record a failed login attempt for the given key.
    ///
    /// The timestamp is kept even when the lockout is disabled (`0`): the adaptive
    /// captcha uses the same failure window as its trigger, and "no lockout" is not
    /// the same policy as "forget that failures happened".
    pub fn record_failure(&self, key: &str) {
        let now = Self::now();
        let mut map = self.lock();
        let timestamps = map.entry(key.to_string()).or_default();
        timestamps.push(now);
        let cutoff = now - self.window();
        timestamps.retain(|&t| t > cutoff);
        Self::shrink_attempts(&mut map);
    }

    /// The number of failures currently inside the window for this attempt's
    /// address or `(address, account)` pair, whichever is larger.
    ///
    /// This is the adaptive-captcha trigger. It deliberately reads the same map as
    /// the lockout, so a successful login that clears the pair/address budget also
    /// clears the trigger.
    pub fn failure_count(&self, keys: &LoginKeys) -> u32 {
        let now = Self::now();
        let cutoff = now - self.window();
        let mut map = self.lock();
        let mut count = 0;
        let mut stale = Vec::new();
        for key in [&keys.ip_key, &keys.pair_key] {
            let Some(timestamps) = map.get_mut(key) else {
                continue;
            };
            timestamps.retain(|&t| t > cutoff);
            if timestamps.is_empty() {
                stale.push(key.clone());
            } else {
                count = count.max(timestamps.len() as u32);
            }
        }
        for key in stale {
            map.remove(&key);
        }
        count
    }

    /// Check if the given key is currently locked out.
    pub fn is_locked(&self, key: &str) -> bool {
        // Without this, `len() >= max_attempts` would reject every login.
        if self.lockout_disabled() {
            return false;
        }
        let now = Self::now();
        let cutoff = now - self.window();
        let mut map = self.lock();
        let (stale, limited) = match map.get_mut(key) {
            Some(timestamps) => {
                timestamps.retain(|&t| t > cutoff);
                (
                    timestamps.is_empty(),
                    timestamps.len() as u32 >= self.limit(),
                )
            }
            None => (false, false),
        };
        if stale {
            // All attempts expired: drop the key so it does not accumulate.
            map.remove(key);
            false
        } else {
            limited
        }
    }

    /// Check whether any of the given keys is currently locked out, using a
    /// single lock acquisition for all keys.
    pub fn is_any_locked(&self, keys: &[&str]) -> bool {
        if self.lockout_disabled() {
            return false;
        }
        let now = Self::now();
        let cutoff = now - self.window();
        let mut map = self.lock();
        for key in keys {
            let (stale, limited) = match map.get_mut(*key) {
                Some(timestamps) => {
                    timestamps.retain(|&t| t > cutoff);
                    (
                        timestamps.is_empty(),
                        timestamps.len() as u32 >= self.limit(),
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
    ///
    /// As in [`Self::record_failure`], timestamps are kept when the lockout is
    /// disabled because the adaptive captcha reads them.
    pub fn record_failures(&self, keys: &[&str]) {
        let now = Self::now();
        let cutoff = now - self.window();
        let mut map = self.lock();
        for key in keys {
            let timestamps = map.entry((*key).to_string()).or_default();
            timestamps.push(now);
            timestamps.retain(|&t| t > cutoff);
        }
        Self::shrink_attempts(&mut map);
    }

    /// Clear all recorded attempts for a key (called on successful login).
    pub fn clear(&self, key: &str) {
        let mut map = self.lock();
        map.remove(key);
    }

    /// Whether this attempt is rejected before credential work: address failures,
    /// (address, account) failures, or failures across too many distinct accounts.
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
    /// Forgive the address and pair counters (otherwise one typo behind a shared NAT locks
    /// out everyone). Keep the distinct-account history: a valid account must not reset
    /// the spray budget.
    pub fn clear_login_failure(&self, keys: &LoginKeys) {
        self.clear(&keys.ip_key);
        self.clear(&keys.pair_key);
    }

    /// Remember that `username` failed from `ip`, for the spray detector.
    fn record_username_failure(&self, ip: &str, username: &str) {
        let now = Self::now();
        let cutoff = now - self.window();
        let mut map = self.spray.lock().unwrap_or_else(PoisonError::into_inner);
        if !map.contains_key(ip) && map.len() >= MAX_SPRAY_IPS {
            // Evict the address whose most recent failure is oldest; timestamps are refreshed on
            // every attempt, so this tracks the least recently active source.
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

    /// Whether `ip` failed on at least the configured number of distinct accounts.
    fn is_spraying(&self, ip: &str) -> bool {
        if self.distinct_limit() == 0 {
            return false;
        }
        let now = Self::now();
        let cutoff = now - self.window();
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
        distinct >= self.distinct_limit()
    }
}

/// As in [`GenericRateLimiter`]: a budget change must not clear earned lockout state.
#[cfg(test)]
mod set_limits_tests {
    use super::LoginRateLimiter;

    #[test]
    fn set_limits_keeps_what_is_already_counted() {
        let limiter = LoginRateLimiter::new(2, 3600, 2);
        limiter.record_failures(&["ip", "ip:pair"]);
        limiter.record_failures(&["ip", "ip:pair"]);
        assert!(limiter.is_locked("ip"));

        limiter.set_limits(4, 3600, 2);
        assert!(
            !limiter.is_locked("ip"),
            "the raised budget applies at once"
        );
        // Two attempts were already counted, so two more reach the new budget of four.
        limiter.record_failures(&["ip"]);
        limiter.record_failures(&["ip"]);
        assert!(limiter.is_locked("ip"));

        limiter.record_username_failure("addr", "a");
        limiter.record_username_failure("addr", "b");
        assert!(limiter.is_spraying("addr"));
        limiter.set_limits(4, 3600, 9);
        assert!(!limiter.is_spraying("addr"));
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

    /// Failures from one address only throttle that address for that account.
    #[test]
    fn pair_key_scopes_failures_to_the_source_address() {
        let limiter = LoginRateLimiter::new(3, 60, 0);
        let attacker = LoginKeys::new("203.0.113.9", "victim@example.com");
        for _ in 0..3 {
            assert!(!limiter.is_login_blocked(&attacker));
            limiter.record_login_failure(&attacker);
        }
        assert!(limiter.is_login_blocked(&attacker));

        let victim = LoginKeys::new("198.51.100.4", "victim@example.com");
        assert!(!limiter.is_login_blocked(&victim));
        limiter.clear_login_failure(&victim);
        assert!(!limiter.is_login_blocked(&victim));

        assert!(!limiter.is_login_blocked(&LoginKeys::new("192.0.2.7", "victim@example.com")));

        // The per-address counter also blocks the attacker's address for other accounts.
        let other = LoginKeys::new("203.0.113.9", "someone-else@example.com");
        assert!(limiter.is_login_blocked(&other));
    }

    /// A success forgives address and pair counters, so a shared NAT is not locked out.
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

    /// Spraying stays blocked across successful logins.
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

    /// A flood of distinct keys must not grow the map: the oldest keys are evicted.
    #[test]
    fn failed_attempt_map_is_capped() {
        let limiter = LoginRateLimiter::new(5, 3600, 0);
        {
            let mut map = limiter.attempts.lock().unwrap();
            for i in 0..(MAX_ATTEMPT_KEYS + 5) {
                map.insert(format!("k{i}"), vec![i as i64]);
            }
        }
        // The trigger's own timestamp is newest, so the injected keys are evicted first.
        limiter.record_failure("trigger");

        let guard = limiter.attempts.lock().unwrap();
        assert!(
            guard.len() <= MAX_ATTEMPT_KEYS,
            "map must stay capped, len={}",
            guard.len()
        );
        for i in 0..5 {
            assert!(
                !guard.contains_key(&format!("k{i}")),
                "k{i} is the oldest and must be evicted"
            );
        }
        assert!(guard.contains_key("trigger"));
    }

    /// The adaptive captcha reads failures even when the lockout itself is off.
    #[test]
    fn failure_count_survives_a_disabled_lockout() {
        let limiter = LoginRateLimiter::new(0, 60, 0);
        let keys = LoginKeys::new("203.0.113.9", "user@example.com");
        for _ in 0..3 {
            limiter.record_login_failure(&keys);
        }
        assert_eq!(limiter.failure_count(&keys), 3);
        assert!(
            !limiter.is_login_blocked(&keys),
            "0 must still mean no lockout"
        );

        limiter.clear_login_failure(&keys);
        assert_eq!(limiter.failure_count(&keys), 0);
    }

    /// The trigger is the larger of the address and pair counters, and expires
    /// with the same window as the lockout.
    #[test]
    fn failure_count_uses_the_max_and_prunes_the_window() {
        let limiter = LoginRateLimiter::new(5, 60, 0);
        let target = LoginKeys::new("203.0.113.9", "victim@example.com");
        let other = LoginKeys::new("203.0.113.9", "other@example.com");
        limiter.record_login_failure(&target);
        limiter.record_login_failure(&other);
        limiter.record_login_failure(&other);
        assert_eq!(
            limiter.failure_count(&target),
            3,
            "the address count is larger than the pair count"
        );

        let old = LoginRateLimiter::now() - 120;
        {
            let mut map = limiter.attempts.lock().unwrap();
            for timestamps in map.values_mut() {
                timestamps.fill(old);
            }
        }
        assert_eq!(limiter.failure_count(&target), 0);
    }
}
