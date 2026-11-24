pub mod generic;
pub mod login;

pub use generic::GenericRateLimiter;
pub use login::{LoginAttempt, LoginKeys, LoginRateLimiter};

use std::collections::HashMap;

/// Timestamp bookkeeping for the fixed-length, sliding-window limiters.
///
/// `GenericRateLimiter` and `LoginRateLimiter` keep the same structure
/// (`Mutex<HashMap<key, Vec<unix-seconds>>>`) and each had its own copy of "drop
/// timestamps outside the window, then compare the count with the limit". A
/// copy could drift on the boundary (`>` vs `>=`, seconds vs duration), which
/// silently changes a security budget; the arithmetic lives here once.
pub(crate) struct SlidingWindow;

impl SlidingWindow {
    /// The oldest timestamp that is still inside the window ending at `now`.
    ///
    /// Attempts at exactly `now - window` are outside: the window is the
    /// half-open interval `(now - window, now]`.
    pub(crate) fn cutoff(now: i64, window_secs: i64) -> i64 {
        now - window_secs
    }

    /// Drop expired timestamps and return the count still inside the window.
    pub(crate) fn prune(timestamps: &mut Vec<i64>, now: i64, window_secs: i64) -> usize {
        let cutoff = Self::cutoff(now, window_secs);
        timestamps.retain(|&timestamp| timestamp > cutoff);
        timestamps.len()
    }

    /// Record one attempt for `key` and return the window count after it.
    pub(crate) fn record(
        map: &mut HashMap<String, Vec<i64>>,
        key: &str,
        now: i64,
        window_secs: i64,
    ) -> usize {
        let timestamps = map.entry(key.to_string()).or_default();
        timestamps.push(now);
        Self::prune(timestamps, now, window_secs)
    }

    /// Count `key`'s attempts inside the window, dropping the key when empty so a
    /// flood of one-off keys does not accumulate zero-length entries.
    pub(crate) fn count(
        map: &mut HashMap<String, Vec<i64>>,
        key: &str,
        now: i64,
        window_secs: i64,
    ) -> usize {
        let Some(timestamps) = map.get_mut(key) else {
            return 0;
        };
        let count = Self::prune(timestamps, now, window_secs);
        if count == 0 {
            map.remove(key);
        }
        count
    }
}

/// Evict the least recently active keys so `map` stays bounded.
///
/// The limiters call this after every recorded attempt, so the common path must
/// stay O(1): the `collect` + `sort` only runs once the map has grown past `max`,
/// and it then trims to 90% of capacity, spreading the cost over the next tenth of
/// inserts. Sorting the whole map under the mutex on *every* insert let a flood of
/// distinct keys turn the limiter into a CPU amplifier.
pub(crate) fn evict_oldest(map: &mut HashMap<String, Vec<i64>>, max: usize) {
    if map.len() <= max {
        return;
    }
    let target = max - max / 10;
    let remove = map.len() - target;
    let mut by_age: Vec<(i64, String)> = map
        .iter()
        .map(|(key, timestamps)| {
            (
                timestamps.iter().copied().max().unwrap_or(i64::MIN),
                key.clone(),
            )
        })
        .collect();
    by_age.sort_unstable();
    for (_, key) in by_age.into_iter().take(remove) {
        map.remove(&key);
    }
}

#[cfg(test)]
mod tests {
    use super::{SlidingWindow, evict_oldest};
    use std::collections::HashMap;

    fn entry(timestamp: i64) -> Vec<i64> {
        vec![timestamp]
    }

    /// The window is half-open `(now - window, now]`: an attempt exactly one
    /// window old is outside, so budgets release on time.
    #[test]
    fn sliding_window_prunes_the_boundary() {
        let mut attempts = vec![90, 100, 101];
        assert_eq!(SlidingWindow::prune(&mut attempts, 100, 10), 2);
        assert_eq!(attempts, vec![100, 101], "90 is exactly at the cutoff");
    }

    #[test]
    fn sliding_window_counts_and_forgets_empty_keys() {
        let mut map: HashMap<String, Vec<i64>> = HashMap::new();
        assert_eq!(SlidingWindow::record(&mut map, "k", 10, 60), 1);
        assert_eq!(SlidingWindow::record(&mut map, "k", 20, 60), 2);
        assert_eq!(SlidingWindow::count(&mut map, "k", 30, 60), 2);
        assert_eq!(SlidingWindow::count(&mut map, "missing", 30, 60), 0);
        // Everything expired: the key is dropped rather than left as an empty Vec.
        assert_eq!(SlidingWindow::count(&mut map, "k", 100, 60), 0);
        assert!(!map.contains_key("k"));
    }

    #[test]
    fn eviction_trims_below_the_limit_so_it_is_amortized() {
        let max = 100;
        let mut map: HashMap<String, Vec<i64>> = (0..=max)
            .map(|i| (format!("k{i}"), entry(i as i64)))
            .collect();
        evict_oldest(&mut map, max);
        assert_eq!(
            map.len(),
            max - max / 10,
            "one batch trim keeps the next tenth of inserts cheap"
        );
        assert!(!map.contains_key("k0"), "the oldest key goes first");
        assert!(map.contains_key(&format!("k{max}")), "the newest key stays");

        // A map at exactly the limit is left alone.
        let mut at_limit: HashMap<String, Vec<i64>> = (0..max)
            .map(|i| (format!("k{i}"), entry(i as i64)))
            .collect();
        evict_oldest(&mut at_limit, max);
        assert_eq!(at_limit.len(), max);
    }
}
