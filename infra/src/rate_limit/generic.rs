/// A generic sliding-window rate limiter for non-login endpoints
/// (password reset, registration, TOTP verification, etc.).
///
/// Tracks attempts per key within a fixed time window.
/// Returns a human-readable message when rate-limited.
use std::collections::HashMap;
use std::sync::{Mutex, MutexGuard, PoisonError};
use std::time::{SystemTime, UNIX_EPOCH};

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

    /// Record an attempt for the given key.
    pub fn record_attempt(&self, key: &str) {
        let now = Self::now();
        let mut map = self.lock();
        let timestamps = map.entry(key.to_string()).or_default();
        timestamps.push(now);
        let cutoff = now - self.window_secs;
        timestamps.retain(|&t| t > cutoff);
    }

    /// Check if the given key has exceeded the rate limit.
    pub fn is_limited(&self, key: &str) -> bool {
        let now = Self::now();
        let cutoff = now - self.window_secs;
        let mut map = self.lock();
        if let Some(timestamps) = map.get_mut(key) {
            timestamps.retain(|&t| t > cutoff);
            timestamps.len() as u32 >= self.max_attempts
        } else {
            false
        }
    }

    /// Clear all recorded attempts for a key.
    pub fn clear(&self, key: &str) {
        let mut map = self.lock();
        map.remove(key);
    }
}
