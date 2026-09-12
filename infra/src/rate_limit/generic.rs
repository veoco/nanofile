/// A generic sliding-window rate limiter for non-login endpoints
/// (password reset, registration, TOTP verification, etc.).
///
/// Tracks attempts per key within a fixed time window.
/// Returns a human-readable message when rate-limited.
///
/// `max_attempts == 0` disables the limiter entirely, matching the documented
/// "(0 = unlimited)" contract of every config field that feeds it. Callers must
/// not silently turn 0 into 1: that would make "unlimited" mean "the first
/// request is already rejected".
use std::collections::HashMap;
use std::sync::{Mutex, MutexGuard, PoisonError};
use std::time::{SystemTime, UNIX_EPOCH};

/// Upper bound on tracked keys.
///
/// Keys are derived from client-controlled values (per-IP, per-link-token) on
/// unauthenticated endpoints, so a flood of distinct keys — e.g. a routed IPv6
/// /64 or a swarm of share tokens — could otherwise grow this map without
/// bound: window expiry only drops a key when that same key is looked up again.
/// Once the cap is reached the least recently active keys are evicted, which
/// fails open (a throttled attacker may regain budget) rather than growing
/// memory until the process dies.
const MAX_KEYS: usize = 50_000;

pub struct GenericRateLimiter {
    attempts: Mutex<HashMap<String, Vec<i64>>>,
    max_attempts: u32,
    window_secs: i64,
}

impl GenericRateLimiter {
    pub fn new(max_attempts: u32, window_secs: u64) -> Self {
        Self {
            attempts: Mutex::new(HashMap::new()),
            max_attempts,
            window_secs: window_secs as i64,
        }
    }

    /// Whether this limiter is disabled (`0 = unlimited`).
    fn disabled(&self) -> bool {
        self.max_attempts == 0
    }

    fn now() -> i64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64
    }

    /// Acquire the internal mutex, recovering from a poisoned state.
    fn lock(&self) -> MutexGuard<'_, HashMap<String, Vec<i64>>> {
        self.attempts.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Bound the map by evicting its least recently active keys.
    ///
    /// Only runs once [`MAX_KEYS`] is exceeded, so the common path stays a
    /// couple of hash operations.
    fn shrink(map: &mut HashMap<String, Vec<i64>>) {
        let Some(excess) = map.len().checked_sub(MAX_KEYS) else {
            return;
        };
        if excess == 0 {
            return;
        }
        let mut by_age: Vec<(i64, String)> = map
            .iter()
            .map(|(key, ts)| (ts.iter().copied().max().unwrap_or(i64::MIN), key.clone()))
            .collect();
        by_age.sort_unstable();
        for (_, key) in by_age.into_iter().take(excess) {
            map.remove(&key);
        }
    }

    /// Record an attempt for the given key.
    pub fn record_attempt(&self, key: &str) {
        if self.disabled() {
            return;
        }
        let now = Self::now();
        let mut map = self.lock();
        let timestamps = map.entry(key.to_string()).or_default();
        timestamps.push(now);
        let cutoff = now - self.window_secs;
        timestamps.retain(|&t| t > cutoff);
        Self::shrink(&mut map);
    }

    /// Check if the given key has exceeded the rate limit.
    pub fn is_limited(&self, key: &str) -> bool {
        if self.disabled() {
            return false;
        }
        let now = Self::now();
        let cutoff = now - self.window_secs;
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

    /// Clear all recorded attempts for a key.
    pub fn clear(&self, key: &str) {
        let mut map = self.lock();
        map.remove(key);
    }
}

#[cfg(test)]
mod tests {
    use super::{GenericRateLimiter, MAX_KEYS};

    #[test]
    fn zero_attempts_means_unlimited() {
        let limiter = GenericRateLimiter::new(0, 3600);
        for _ in 0..100 {
            limiter.record_attempt("k");
        }
        assert!(
            !limiter.is_limited("k"),
            "0 must mean unlimited, not 'the first request is rejected'"
        );
        // Nothing is recorded either, so the map cannot grow for a disabled
        // limiter.
        assert!(limiter.attempts.lock().unwrap().is_empty());
    }

    #[test]
    fn positive_limit_still_applies() {
        let limiter = GenericRateLimiter::new(2, 3600);
        limiter.record_attempt("k");
        assert!(!limiter.is_limited("k"));
        limiter.record_attempt("k");
        assert!(limiter.is_limited("k"));
        limiter.clear("k");
        assert!(!limiter.is_limited("k"));
    }

    /// A flood of distinct keys must not grow the map without bound: eviction
    /// keeps the least recently active keys out.
    #[test]
    fn shrink_bounds_the_map_and_drops_the_oldest_keys() {
        let limiter = GenericRateLimiter::new(5, 3600);
        {
            let mut map = limiter.attempts.lock().unwrap();
            for i in 0..(MAX_KEYS + 5) {
                map.insert(format!("k{i}"), vec![i as i64]);
            }
        }
        // A normal attempt triggers the shrink without changing the ordering:
        // its timestamp is "now", far newer than every injected one.
        limiter.record_attempt("trigger");

        let guard = limiter.attempts.lock().unwrap();
        assert!(
            guard.len() <= MAX_KEYS,
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
}
