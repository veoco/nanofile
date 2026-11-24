//! In-memory login attempt rate limiter.
//!
//! Tracks failures by key (IP or `ip:username`), locks out after a threshold in a
//! window, and counts distinct failed accounts per address so credential spraying is
//! caught even when a success keeps resetting the per-address counter.
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

/// One mutex covering both maps, so "check the budget and reserve an in-flight
/// attempt" is atomic. Two separate mutexes would let a burst pass the check
/// before any failure is recorded (the race this type exists to close), and
/// acquiring them in different orders from the guard's release path could
/// deadlock.
#[derive(Default)]
struct LoginState {
    /// Recorded failures per key (`ip` and `ip:account`), inside the window.
    failures: HashMap<String, Vec<i64>>,
    /// Attempts currently being verified, per key. A reservation bounds how many
    /// password checks can be in flight at once, which is what closes the
    /// check-then-verify window: PBKDF2 takes long enough that an unbounded burst
    /// would otherwise all pass the failure check before the first failure is
    /// recorded. Reservations are released on success, failure or drop, so a
    /// successful login still consumes no failure budget.
    in_flight: HashMap<String, u32>,
}

pub struct LoginRateLimiter {
    state: Mutex<LoginState>,
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
            state: Mutex::new(LoginState::default()),
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

    fn limit(&self) -> u32 {
        self.max_attempts.load(Ordering::Relaxed)
    }

    fn window(&self) -> i64 {
        self.lockout_secs.load(Ordering::Relaxed)
    }

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

    /// Acquire the mutex, recovering from poison: neither map has an invariant a
    /// panic elsewhere could violate.
    fn lock(&self) -> MutexGuard<'_, LoginState> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Evict the least recently active keys once [`MAX_ATTEMPT_KEYS`] is exceeded, so the
    /// common path stays a couple of hash operations.
    fn shrink_attempts(state: &mut LoginState) {
        super::evict_oldest(&mut state.failures, MAX_ATTEMPT_KEYS);
    }

    /// Record a failed login attempt for the given key.
    ///
    /// The timestamp is kept even when the lockout is disabled (`0`): the adaptive
    /// captcha uses the same failure window as its trigger, and "no lockout" is not
    /// the same policy as "forget that failures happened".
    pub fn record_failure(&self, key: &str) {
        let now = Self::now();
        let mut state = self.lock();
        super::SlidingWindow::record(&mut state.failures, key, now, self.window());
        Self::shrink_attempts(&mut state);
    }

    /// The number of failures currently inside the window for this attempt's
    /// address or `(address, account)` pair, whichever is larger.
    ///
    /// This is the adaptive-captcha trigger. It deliberately reads the same map as
    /// the lockout, so a successful login that clears the pair/address budget also
    /// clears the trigger. In-flight reservations are not failures and are not
    /// counted here.
    pub fn failure_count(&self, keys: &LoginKeys) -> u32 {
        let now = Self::now();
        let mut state = self.lock();
        let mut count = 0;
        for key in [&keys.ip_key, &keys.pair_key] {
            let key_count =
                super::SlidingWindow::count(&mut state.failures, key, now, self.window()) as u32;
            count = count.max(key_count);
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
        let mut state = self.lock();
        super::SlidingWindow::count(&mut state.failures, key, now, self.window()) as u32
            >= self.limit()
    }

    /// Check whether any of the given keys is currently locked out, using a
    /// single lock acquisition for all keys.
    pub fn is_any_locked(&self, keys: &[&str]) -> bool {
        if self.lockout_disabled() {
            return false;
        }
        let now = Self::now();
        let mut state = self.lock();
        for key in keys {
            if super::SlidingWindow::count(&mut state.failures, key, now, self.window()) as u32
                >= self.limit()
            {
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
        let mut state = self.lock();
        for key in keys {
            super::SlidingWindow::record(&mut state.failures, key, now, self.window());
        }
        Self::shrink_attempts(&mut state);
    }

    /// Clear all recorded attempts for a key (called on successful login).
    pub fn clear(&self, key: &str) {
        let mut state = self.lock();
        state.failures.remove(key);
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

    /// Atomically check the budget and reserve one in-flight attempt, or return
    /// `None` when this attempt must be refused immediately.
    ///
    /// This is the login path's entry point. Doing the check and the reservation
    /// under one lock is what stops a concurrent burst from passing the failure
    /// check before the first failure lands: the check and the expensive password
    /// verification are separated by a lock-acquisition-sized window instead of a
    /// PBKDF2-sized one.
    ///
    /// The reservation itself is not a failure: [`LoginAttempt::succeed`] releases
    /// it and clears the address/pair counters as before, so a shared NAT is not
    /// punished for simultaneous successful logins. The in-flight ceiling is
    /// deliberately wider than the failure budget (`4 x`, clamped to
    /// `4..=64`), so ordinary simultaneous logins from one address still fit while
    /// an unbounded burst cannot.
    pub fn begin_login_attempt(&self, keys: &LoginKeys) -> Option<LoginAttempt<'_>> {
        let now = Self::now();
        let mut state = self.lock();

        // Recorded failures: the same check `is_login_blocked` makes, including
        // pruning keys whose window has emptied.
        if !self.lockout_disabled() {
            let limit = self.limit();
            for key in [&keys.ip_key, &keys.pair_key] {
                if super::SlidingWindow::count(&mut state.failures, key, now, self.window()) as u32
                    >= limit
                {
                    return None;
                }
            }
        }

        // The spray detector is independent of the lockout switch.
        if self.is_spraying(&keys.client_ip) {
            return None;
        }

        // Reserve against the in-flight caps. The lockout being disabled (`0`)
        // also disables the in-flight cap: a host that switched the lockout off
        // asked for no throttling, and the cap exists only to protect the budget.
        let reserved = if self.lockout_disabled() {
            false
        } else {
            let limit = self.limit();
            let ip_cap = limit.saturating_mul(4).clamp(4, 64);
            let pair_cap = limit.max(1);
            let ip_in_flight = state.in_flight.get(&keys.ip_key).copied().unwrap_or(0);
            let pair_in_flight = state.in_flight.get(&keys.pair_key).copied().unwrap_or(0);
            if ip_in_flight >= ip_cap || pair_in_flight >= pair_cap {
                return None;
            }
            *state.in_flight.entry(keys.ip_key.clone()).or_default() += 1;
            *state.in_flight.entry(keys.pair_key.clone()).or_default() += 1;
            true
        };

        Some(LoginAttempt {
            limiter: self,
            keys: keys.clone(),
            reserved,
            completed: false,
        })
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

/// One login attempt that passed the rate-limit gate and holds an in-flight
/// reservation.
///
/// The caller must end it with [`LoginAttempt::fail`] (credential failure) or
/// [`LoginAttempt::succeed`] (correct credentials, even if a later step such as
/// 2FA still stops the login). Dropping it releases the reservation without
/// recording either outcome, which is what a database error or a client
/// disconnect should do.
pub struct LoginAttempt<'a> {
    limiter: &'a LoginRateLimiter,
    keys: LoginKeys,
    /// False when the lockout is disabled: nothing was reserved, so there is
    /// nothing to release.
    reserved: bool,
    completed: bool,
}

impl LoginAttempt<'_> {
    /// This attempt used a wrong password (or a wrong second factor): count it
    /// against the address, the pair and the distinct-account spray detector.
    pub fn fail(&mut self) {
        if self.completed {
            return;
        }
        self.completed = true;
        self.release_reservation();
        self.limiter
            .record_failures(&[self.keys.ip_key.as_str(), self.keys.pair_key.as_str()]);
        self.limiter
            .record_username_failure(&self.keys.client_ip, &self.keys.username);
    }

    /// The password was correct: forgive the address/pair counters exactly as the
    /// pre-reservation code did. The spray history is intentionally kept.
    pub fn succeed(&mut self) {
        if self.completed {
            return;
        }
        self.completed = true;
        self.release_reservation();
        self.limiter.clear_login_failure(&self.keys);
    }

    fn release_reservation(&self) {
        if !self.reserved {
            return;
        }
        let mut state = self.limiter.lock();
        for key in [&self.keys.ip_key, &self.keys.pair_key] {
            let Some(count) = state.in_flight.get_mut(key) else {
                continue;
            };
            if *count <= 1 {
                state.in_flight.remove(key);
            } else {
                *count -= 1;
            }
        }
    }
}

impl Drop for LoginAttempt<'_> {
    fn drop(&mut self) {
        if !self.completed {
            self.release_reservation();
        }
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
            .state
            .lock()
            .unwrap()
            .failures
            .insert("stale".to_string(), vec![old]);
        assert!(!limiter.is_locked("stale"));
        assert!(!limiter.state.lock().unwrap().failures.contains_key("stale"));
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
            let mut state = limiter.state.lock().unwrap();
            for i in 0..(MAX_ATTEMPT_KEYS + 5) {
                state.failures.insert(format!("k{i}"), vec![i as i64]);
            }
        }
        // The trigger's own timestamp is newest, so the injected keys are evicted first.
        limiter.record_failure("trigger");

        let state = limiter.state.lock().unwrap();
        assert!(
            state.failures.len() <= MAX_ATTEMPT_KEYS,
            "map must stay capped, len={}",
            state.failures.len()
        );
        for i in 0..5 {
            assert!(
                !state.failures.contains_key(&format!("k{i}")),
                "k{i} is the oldest and must be evicted"
            );
        }
        assert!(state.failures.contains_key("trigger"));
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
            let mut state = limiter.state.lock().unwrap();
            for timestamps in state.failures.values_mut() {
                timestamps.fill(old);
            }
        }
        assert_eq!(limiter.failure_count(&target), 0);
    }

    /// A burst that arrives before any failure is recorded must not all reach the
    /// password verification: the reservation is taken under the same lock as the
    /// check. Successful attempts hold their reservation until this test releases
    /// them (the second barrier), so scheduling cannot turn the cap into a race.
    #[test]
    fn concurrent_attempts_are_capped_before_any_failure_is_recorded() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        use std::sync::{Arc, Barrier};

        const THREADS: usize = 32;
        let limiter = Arc::new(LoginRateLimiter::new(5, 60, 0));
        let keys = LoginKeys::new("203.0.113.9", "victim@example.com");
        // Two phases: all start together; each holds its guard until every thread
        // has attempted the reservation, then releases.
        let barrier = Arc::new(Barrier::new(THREADS));
        let granted = Arc::new(AtomicUsize::new(0));

        let handles: Vec<_> = (0..THREADS)
            .map(|_| {
                let limiter = limiter.clone();
                let keys = keys.clone();
                let barrier = barrier.clone();
                let granted = granted.clone();
                std::thread::spawn(move || {
                    barrier.wait();
                    let guard = limiter.begin_login_attempt(&keys);
                    if guard.is_some() {
                        granted.fetch_add(1, Ordering::SeqCst);
                    }
                    barrier.wait();
                    drop(guard);
                })
            })
            .collect();
        for handle in handles {
            handle.join().expect("attempt thread");
        }

        // The per-pair cap follows the failure budget; the address cap is wider.
        assert_eq!(
            granted.load(Ordering::SeqCst),
            5,
            "only the failure budget's worth of attempts may be in flight"
        );
    }

    /// A successful attempt releases its reservation and clears the failure
    /// budget, so a working user is never throttled by earlier typos.
    #[test]
    fn success_releases_the_reservation_without_recording_a_failure() {
        let limiter = LoginRateLimiter::new(2, 60, 0);
        let keys = LoginKeys::new("198.51.100.4", "user@example.com");

        for _ in 0..5 {
            let mut attempt = limiter
                .begin_login_attempt(&keys)
                .expect("a successful attempt must be admitted under the budget");
            attempt.succeed();
            assert_eq!(limiter.failure_count(&keys), 0);
        }
        assert!(!limiter.is_login_blocked(&keys));
    }

    /// Dropping an attempt only releases the reservation: a database error or a
    /// client disconnect is not a credential failure.
    #[test]
    fn dropping_an_attempt_records_nothing_and_frees_the_slot() {
        let limiter = LoginRateLimiter::new(1, 60, 0);
        let keys = LoginKeys::new("198.51.100.4", "user@example.com");

        drop(limiter.begin_login_attempt(&keys));
        assert_eq!(limiter.failure_count(&keys), 0);
        // The single pair slot is free again.
        let mut attempt = limiter
            .begin_login_attempt(&keys)
            .expect("the dropped attempt must not hold the slot");
        attempt.succeed();
    }

    /// A failed attempt records against the address and the pair, and holding the
    /// guard's failure consumes the budget: the next attempt is refused without
    /// waiting for any window.
    #[test]
    fn a_failed_attempt_consumes_the_budget_immediately() {
        let limiter = LoginRateLimiter::new(2, 60, 0);
        let keys = LoginKeys::new("198.51.100.4", "user@example.com");

        let mut first = limiter.begin_login_attempt(&keys).expect("first attempt");
        first.fail();
        let mut second = limiter.begin_login_attempt(&keys).expect("second attempt");
        second.fail();

        assert!(limiter.is_login_blocked(&keys));
        assert!(limiter.begin_login_attempt(&keys).is_none());
    }

    /// `0 = unlimited` also switches the in-flight cap off: the switch means no
    /// throttling, not "throttle without recording failures".
    #[test]
    fn disabled_lockout_does_not_cap_in_flight_attempts() {
        let limiter = LoginRateLimiter::new(0, 60, 0);
        let keys = LoginKeys::new("203.0.113.9", "user@example.com");
        let mut attempts: Vec<_> = (0..20)
            .map(|_| {
                limiter
                    .begin_login_attempt(&keys)
                    .expect("lockout disabled means no cap")
            })
            .collect();
        for attempt in &mut attempts {
            attempt.succeed();
        }
    }

    /// A record whose failure budget is already spent refuses the next attempt
    /// even while no other request is in flight.
    #[test]
    fn recorded_failures_are_checked_by_begin() {
        let limiter = LoginRateLimiter::new(2, 60, 0);
        let keys = LoginKeys::new("203.0.113.9", "victim@example.com");
        limiter.record_failures(&[keys.ip_key.as_str(), keys.pair_key.as_str()]);
        limiter.record_failures(&[keys.ip_key.as_str(), keys.pair_key.as_str()]);
        assert!(limiter.begin_login_attempt(&keys).is_none());
    }
}
