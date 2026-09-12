//! Per-server caches backing storage-quota accounting.
//!
//! Two pieces of state live here:
//!
//! * **Committed-usage snapshots** — `SUM(repo.size)` per user, cached for a
//!   short TTL so the hot upload paths (`put_block`, every upload chunk) do not
//!   run a full-table aggregate.
//! * **Uncommitted-block reservations** — bytes a user has written to the
//!   content-addressed block store that no commit references yet. Content
//!   addressing means a block write never changes any repo's `size`, so without
//!   this counter a client could write blocks forever without ever committing
//!   and never touch a quota.
//!
//! Both are deliberately owned by a [`crate::…`] `Repositories` instance rather
//! than being process-global statics: they must describe *one* database, and a
//! process can host several servers (each integration test starts its own, with
//! its own user ids starting at 1). Process-global maps keyed by user id would
//! leak one server's usage and reservations into another's.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// How long a committed-usage snapshot stays valid.
///
/// `repo.size` only changes on commit, which is a low-frequency path compared
/// to the per-chunk checks that read this cache, and commits invalidate the
/// entry outright, so a short TTL is only a backstop.
const USAGE_TTL: Duration = Duration::from_millis(500);

/// Hard cap on cached usage entries so a flush of distinct users cannot grow
/// the map without bound.
const USAGE_MAX: usize = 4096;

/// Hard cap on tracked `(user, repo)` reservations.
const RESERVED_MAX: usize = 8192;

/// Reason a reservation was refused: it would push the user past their quota.
///
/// A dedicated type rather than `()` so the outcome is self-describing at the
/// call site (`map_err(|_| AppError::QuotaExceeded)`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct QuotaExceeded;

#[derive(Default)]
pub struct QuotaCache {
    usage: Mutex<HashMap<i32, (i64, Instant)>>,
    reserved: Mutex<HashMap<(i32, String), i64>>,
}

impl QuotaCache {
    pub fn new() -> Self {
        Self::default()
    }

    /// A cached committed usage for `user_id`, if it is still fresh.
    pub fn cached_usage(&self, user_id: i32) -> Option<i64> {
        let now = Instant::now();
        let map = self.usage.lock().unwrap_or_else(|e| e.into_inner());
        match map.get(&user_id) {
            Some((usage, at)) if now.duration_since(*at) < USAGE_TTL => Some(*usage),
            _ => None,
        }
    }

    /// Store a freshly computed committed usage.
    pub fn store_usage(&self, user_id: i32, usage: i64) {
        let mut map = self.usage.lock().unwrap_or_else(|e| e.into_inner());
        if map.len() >= USAGE_MAX {
            let now = Instant::now();
            map.retain(|_, (_, at)| now.duration_since(*at) < USAGE_TTL);
        }
        if map.len() < USAGE_MAX {
            map.insert(user_id, (usage, Instant::now()));
        }
    }

    /// Drop a user's usage snapshot, so the next check re-reads the database.
    ///
    /// Commits call this: without it a commit leaves the 500 ms snapshot in
    /// place, and several uploads started inside that window all see the
    /// pre-commit usage — a check-then-write race that lets the aggregate
    /// exceed the quota by roughly the number of concurrent uploads.
    pub fn invalidate_usage(&self, user_id: i32) {
        let mut map = self.usage.lock().unwrap_or_else(|e| e.into_inner());
        map.remove(&user_id);
    }

    /// Bytes reserved by `user_id` across every repository.
    pub fn reserved_total(&self, user_id: i32) -> i64 {
        let map = self.reserved.lock().unwrap_or_else(|e| e.into_inner());
        map.iter()
            .filter(|((uid, _), _)| *uid == user_id)
            .map(|(_, n)| *n)
            .sum()
    }

    /// Atomically reserve `delta` more bytes for `(user_id, repo_id)` when
    /// `committed + already_reserved + delta <= quota`.
    ///
    /// Returns [`QuotaExceeded`] when the reservation would exceed the quota;
    /// nothing is reserved in that case. The check and the insert happen under
    /// one lock so concurrent requests cannot both consume the same headroom.
    pub fn reserve_if_fits(
        &self,
        user_id: i32,
        repo_id: &str,
        delta: i64,
        committed: i64,
        quota: i64,
    ) -> Result<(), QuotaExceeded> {
        if delta <= 0 {
            return Ok(());
        }
        let mut map = self.reserved.lock().unwrap_or_else(|e| e.into_inner());

        let reserved: i64 = map
            .iter()
            .filter(|((uid, _), _)| *uid == user_id)
            .map(|(_, n)| *n)
            .sum();
        if committed + reserved + delta > quota {
            return Err(QuotaExceeded);
        }

        let key = (user_id, repo_id.to_string());
        if map.len() >= RESERVED_MAX && !map.contains_key(&key) {
            // Bound the map: released keys are removed outright, so in practice
            // only live uploads are tracked.
            map.retain(|_, n| *n > 0);
            if map.len() >= RESERVED_MAX {
                // Fail open on accounting megadata rather than rejecting a
                // legitimate upload: quota is a fairness control, and the
                // per-request body limit still bounds a single write.
                return Ok(());
            }
        }
        *map.entry(key).or_insert(0) += delta;
        Ok(())
    }

    /// Drop every outstanding reservation for `(user_id, repo_id)`.
    ///
    /// Called when an upload commits (the bytes are now counted by the
    /// committed-usage snapshot) or when the blocks are deleted again.
    pub fn release_repo(&self, user_id: i32, repo_id: &str) -> i64 {
        let mut map = self.reserved.lock().unwrap_or_else(|e| e.into_inner());
        map.remove(&(user_id, repo_id.to_string())).unwrap_or(0)
    }
}

#[cfg(test)]
mod tests {
    use super::QuotaCache;

    #[test]
    fn reservations_are_per_user_and_per_repo() {
        let cache = QuotaCache::new();
        assert!(cache.reserve_if_fits(1, "r1", 10, 0, 1000).is_ok());
        assert!(cache.reserve_if_fits(1, "r2", 5, 0, 1000).is_ok());
        assert!(cache.reserve_if_fits(2, "r1", 7, 0, 1000).is_ok());

        assert_eq!(cache.reserved_total(1), 15);
        assert_eq!(cache.reserved_total(2), 7);

        assert_eq!(cache.release_repo(1, "r1"), 10);
        assert_eq!(cache.reserved_total(1), 5, "only r1 was released");
        assert_eq!(cache.reserved_total(2), 7, "other users untouched");
    }

    #[test]
    fn reservation_is_refused_when_committed_plus_reserved_exceeds_quota() {
        let cache = QuotaCache::new();
        // 900 committed, 700 reserved: 700 more would total 2300 > 2000.
        assert!(cache.reserve_if_fits(1, "r1", 700, 900, 2000).is_ok());
        assert!(
            cache.reserve_if_fits(1, "r1", 700, 900, 2000).is_err(),
            "the second reservation must not fit"
        );
        // The refused reservation left nothing behind.
        assert_eq!(cache.reserved_total(1), 700);
    }

    #[test]
    fn commit_invalidates_the_cached_usage() {
        let cache = QuotaCache::new();
        assert_eq!(cache.cached_usage(1), None);
        cache.store_usage(1, 42);
        assert_eq!(cache.cached_usage(1), Some(42));
        cache.invalidate_usage(1);
        assert_eq!(cache.cached_usage(1), None);
    }

    #[test]
    fn instances_do_not_share_state() {
        // The reason this type is not a process-global static: two servers in
        // one process must not see each other's usage or reservations.
        let a = QuotaCache::new();
        let b = QuotaCache::new();
        a.store_usage(1, 100);
        assert!(a.reserve_if_fits(1, "r1", 10, 0, 1000).is_ok());
        assert_eq!(b.cached_usage(1), None);
        assert_eq!(b.reserved_total(1), 0);
    }
}
