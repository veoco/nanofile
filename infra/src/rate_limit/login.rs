/// In-memory login attempt rate limiter.
///
/// Tracks failed login attempts by key (IP or "ip:username") and
/// prevents further attempts after a configurable threshold within
/// a configurable time window.
use std::collections::HashMap;
use std::sync::{Mutex, MutexGuard, PoisonError};
use std::time::{SystemTime, UNIX_EPOCH};

pub struct LoginRateLimiter {
    attempts: Mutex<HashMap<String, Vec<i64>>>,
    max_attempts: u32,
    lockout_secs: i64,
}

impl LoginRateLimiter {
    pub fn new(max_attempts: u32, lockout_secs: u64) -> Self {
        Self {
            attempts: Mutex::new(HashMap::new()),
            max_attempts,
            lockout_secs: lockout_secs as i64,
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
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_allows_first_attempt() {
        let limiter = LoginRateLimiter::new(3, 60);
        assert!(!limiter.is_locked("test-user"));
    }

    #[test]
    fn test_locks_after_threshold() {
        let limiter = LoginRateLimiter::new(3, 60);
        limiter.record_failure("test-user");
        limiter.record_failure("test-user");
        limiter.record_failure("test-user");
        assert!(limiter.is_locked("test-user"));
    }

    #[test]
    fn test_clear_resets() {
        let limiter = LoginRateLimiter::new(3, 60);
        limiter.record_failure("test-user");
        limiter.record_failure("test-user");
        limiter.record_failure("test-user");
        assert!(limiter.is_locked("test-user"));
        limiter.clear("test-user");
        assert!(!limiter.is_locked("test-user"));
    }

    #[test]
    fn test_allows_below_threshold() {
        let limiter = LoginRateLimiter::new(5, 60);
        limiter.record_failure("test-user");
        limiter.record_failure("test-user");
        assert!(!limiter.is_locked("test-user"));
    }

    #[test]
    fn test_is_any_locked() {
        let limiter = LoginRateLimiter::new(3, 60);
        limiter.record_failure("key-a");
        limiter.record_failure("key-a");
        limiter.record_failure("key-a");
        assert!(limiter.is_any_locked(&["key-a", "key-b"]));
        assert!(!limiter.is_any_locked(&["key-b"]));
    }

    #[test]
    fn test_record_failures_batch() {
        let limiter = LoginRateLimiter::new(3, 60);
        limiter.record_failures(&["key-a", "key-b"]);
        limiter.record_failures(&["key-a", "key-b"]);
        assert!(!limiter.is_locked("key-a"));
        limiter.record_failures(&["key-a", "key-b"]);
        assert!(limiter.is_any_locked(&["key-a"]));
    }

    #[test]
    fn test_expired_key_is_pruned() {
        let limiter = LoginRateLimiter::new(3, 60);
        let old = LoginRateLimiter::now() - 120;
        limiter
            .attempts
            .lock()
            .unwrap()
            .insert("stale".to_string(), vec![old]);
        assert!(!limiter.is_locked("stale"));
        assert!(!limiter.attempts.lock().unwrap().contains_key("stale"));
    }
}
