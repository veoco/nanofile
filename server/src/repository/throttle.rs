//! Rate limiting for the "when was this last used?" columns.
//!
//! `api_keys.last_used_at` and `sync_tokens.last_sync_time` are read by a human
//! and written on a hot path: a key authenticates every request its client
//! makes, and a syncing client makes many small ones. SQLite serialises writes,
//! so an unthrottled update per request is a real cost for a timestamp whose
//! accuracy does not matter to the minute.
//!
//! The state lives on [`Repositories`](super::Repositories) rather than in a
//! process-global, because it describes writes to *that* database. A global
//! would be wrong the moment two independent databases are opened in one
//! process — which is exactly what the test suite does, and which showed up as
//! a key never recording a use because an unrelated test's key had the same id.

use std::collections::BTreeMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// How often a sync token's peer info may be persisted at most, so
/// `last_sync_time` stays within about a minute of the truth.
pub const PEER_INFO_WRITE_INTERVAL: Duration = Duration::from_secs(60);

/// How often an API key's `last_used_at` may be persisted at most.
pub const KEY_USAGE_WRITE_INTERVAL: Duration = Duration::from_secs(60);

/// At most one write per id per interval.
pub struct WriteThrottle {
    interval: Duration,
    /// Last write per id. Entries older than the interval are pruned on insert,
    /// so the map stays bounded to the ids seen within one interval. A
    /// `BTreeMap` rather than a `HashMap` only because it has a `const`
    /// constructor, which is what lets this be built in a `const`.
    last: Mutex<BTreeMap<i32, Instant>>,
}

impl WriteThrottle {
    pub const fn new(interval: Duration) -> Self {
        Self {
            interval,
            last: Mutex::new(BTreeMap::new()),
        }
    }

    /// Whether `id` may be written now, recording the write when it may.
    ///
    /// `now` is a parameter rather than read from the clock so the behaviour is
    /// testable without sleeping.
    pub fn should_write(&self, id: i32, now: Instant) -> bool {
        let mut last = self.last.lock().unwrap_or_else(|e| e.into_inner());
        if last
            .get(&id)
            .is_some_and(|previous| now.duration_since(*previous) < self.interval)
        {
            return false;
        }
        last.retain(|_, previous| now.duration_since(*previous) < self.interval);
        last.insert(id, now);
        true
    }

    /// [`should_write`](Self::should_write) against the current clock.
    pub fn allows_now(&self, id: i32) -> bool {
        self.should_write(id, Instant::now())
    }

    /// How many ids are currently tracked. Exposed for the tests that pin the
    /// pruning that keeps the map bounded.
    #[cfg(test)]
    fn tracked(&self) -> usize {
        self.last.lock().unwrap_or_else(|e| e.into_inner()).len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The throttle is what keeps a "last used" column from turning every
    /// request into a write.
    #[test]
    fn one_write_per_interval() {
        let throttle = WriteThrottle::new(Duration::from_secs(60));
        let start = Instant::now();

        assert!(throttle.should_write(1, start), "the first write goes");
        assert!(
            !throttle.should_write(1, start + Duration::from_secs(59)),
            "a second write inside the interval must not reach the database"
        );
        assert!(
            throttle.should_write(1, start + Duration::from_secs(61)),
            "once the interval has passed the write goes again"
        );

        // Ids are throttled independently: a busy sync token must not suppress
        // a key's usage record.
        assert!(throttle.should_write(2, start));
        assert!(!throttle.should_write(1, start + Duration::from_secs(62)));
    }

    /// The map has to stay bounded, or a long-running server accumulates one
    /// entry per credential it has ever seen.
    #[test]
    fn ids_that_aged_out_are_forgotten() {
        let throttle = WriteThrottle::new(Duration::from_secs(60));
        let start = Instant::now();
        for id in 0..100 {
            assert!(throttle.should_write(id, start));
        }
        assert_eq!(throttle.tracked(), 100);

        // One write a full interval later prunes everything from before it.
        assert!(throttle.should_write(0, start + Duration::from_secs(61)));
        assert_eq!(throttle.tracked(), 1, "only the id just written is kept");
    }

    /// Two databases in one process must not throttle each other: their ids are
    /// unrelated, so "row 1 was just written" is not a fact about both.
    #[test]
    fn throttles_do_not_share_state() {
        let first = WriteThrottle::new(KEY_USAGE_WRITE_INTERVAL);
        let second = WriteThrottle::new(KEY_USAGE_WRITE_INTERVAL);
        let now = Instant::now();

        assert!(first.should_write(1, now));
        assert!(
            second.should_write(1, now),
            "a separate database starts with its own history"
        );
    }
}
