/// A generic sliding-window rate limiter for non-login endpoints (password reset,
/// registration, TOTP verification, …).
///
/// Tracks attempts per key within a window and returns a human-readable message when
/// limited. `max_attempts == 0` disables it, matching every config field's
/// "(0 = unlimited)"; callers must not turn 0 into 1.
use std::collections::HashMap;
use std::sync::atomic::{AtomicI64, AtomicU32, Ordering};
use std::sync::{Mutex, MutexGuard, PoisonError};
use std::time::{SystemTime, UNIX_EPOCH};

/// Cap on tracked keys. Keys come from client-controlled values on unauthenticated
/// endpoints, so a flood (IPv6 /64, share tokens) could grow the map; window expiry
/// only drops a key when it is looked up again. Once full, the least recently active
/// keys are evicted — failing open rather than growing memory until the process dies.
const MAX_KEYS: usize = 50_000;

pub struct GenericRateLimiter {
    attempts: Mutex<HashMap<String, Vec<i64>>>,
    /// Atomic so the budget can change on a running server; counted attempts are kept, so
    /// raising the limit does not clear lockouts.
    max_attempts: AtomicU32,
    window_secs: AtomicI64,
}

impl GenericRateLimiter {
    pub fn new(max_attempts: u32, window_secs: u64) -> Self {
        Self {
            attempts: Mutex::new(HashMap::new()),
            max_attempts: AtomicU32::new(max_attempts),
            window_secs: AtomicI64::new(window_secs as i64),
        }
    }

    /// Replace the budget and the window without touching what is counted.
    pub fn set_limits(&self, max_attempts: u32, window_secs: u64) {
        self.max_attempts.store(max_attempts, Ordering::Relaxed);
        self.window_secs
            .store(window_secs as i64, Ordering::Relaxed);
    }

    /// Whether this limiter is disabled (`0 = unlimited`).
    fn disabled(&self) -> bool {
        self.max_attempts.load(Ordering::Relaxed) == 0
    }

    /// The configured budget.
    fn limit(&self) -> u32 {
        self.max_attempts.load(Ordering::Relaxed)
    }

    /// The configured window, in seconds.
    fn window(&self) -> i64 {
        self.window_secs.load(Ordering::Relaxed)
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

    /// Evict the least recently active keys once [`MAX_KEYS`] is exceeded, keeping the
    /// common path to a couple of hash operations.
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
        let cutoff = now - self.window();
        timestamps.retain(|&t| t > cutoff);
        Self::shrink(&mut map);
    }

    /// Check if the given key has exceeded the rate limit.
    pub fn is_limited(&self, key: &str) -> bool {
        if self.disabled() {
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

    /// Clear all recorded attempts for a key.
    pub fn clear(&self, key: &str) {
        let mut map = self.lock();
        map.remove(key);
    }
}

#[cfg(test)]
mod tests {
    use super::{GenericRateLimiter, MAX_KEYS};

    /// Raising the budget takes effect immediately without forgetting who has been trying:
    /// a save that cleared counters would clear every lockout.
    #[test]
    fn set_limits_keeps_what_is_already_counted() {
        let limiter = GenericRateLimiter::new(2, 3600);
        limiter.record_attempt("k");
        limiter.record_attempt("k");
        assert!(
            limiter.is_limited("k"),
            "the second attempt hits the budget"
        );

        limiter.set_limits(3, 3600);
        assert!(
            !limiter.is_limited("k"),
            "the raised budget applies at once, over the two attempts already counted"
        );
        limiter.record_attempt("k");
        assert!(limiter.is_limited("k"), "the third attempt now hits it");

        limiter.set_limits(0, 3600);
        assert!(!limiter.is_limited("k"), "0 still means unlimited");
    }

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
        // Disabled limiters record nothing, so the map cannot grow.
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

    /// A flood of distinct keys must not grow the map: the oldest are evicted.
    #[test]
    fn shrink_bounds_the_map_and_drops_the_oldest_keys() {
        let limiter = GenericRateLimiter::new(5, 3600);
        {
            let mut map = limiter.attempts.lock().unwrap();
            for i in 0..(MAX_KEYS + 5) {
                map.insert(format!("k{i}"), vec![i as i64]);
            }
        }
        // The trigger's timestamp is newest, so injected keys are evicted first.
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
