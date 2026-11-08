//! Per-server caches backing storage-quota accounting.
//!
//! * Committed-usage snapshots: `SUM(repo.size)` per user, cached briefly so the
//!   hot upload paths do not run a full-table aggregate.
//! * Uncommitted-block reservations: bytes written to the content-addressed block
//!   store that no commit references. A block write never changes `repo.size`, so
//!   without this a client could write forever without committing, never touching
//!   a quota.
//! * A per-user async lock. Quota checks and the commit that consumes the headroom
//!   must run under the same lock, or two concurrent commits both observe the
//!   pre-commit usage and overshoot.
//!
//! Both maps belong to one `Repositories` instance, not to process-global statics: a
//! process can host several servers (integration tests each start one), and global
//! maps keyed by user id would leak usage between them.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// How long a committed-usage snapshot stays valid. `repo.size` changes only on
/// commit, which invalidates the entry outright, so the TTL is only a backstop for
/// the per-chunk checks.
const USAGE_TTL: Duration = Duration::from_millis(500);

/// Cap on cached usage entries.
const USAGE_MAX: usize = 4096;

/// Hard cap on tracked reservation **groups** (one per temp upload, one per
/// `(user, repo)` block batch) across all users. A large file has many blocks but
/// still one group, so this bounds bookkeeping memory without capping file size.
const RESERVED_MAX: usize = 8192;

/// Cap on tracked reservation groups for one user. One account must not be able to
/// fill the global table and make every other account fail closed.
const RESERVED_MAX_PER_USER: usize = 1024;

/// How long an uncommitted reservation may live without being released. A sync or
/// block upload that never commits would otherwise reduce the account's headroom
/// until the process restarts. Refreshed on each block or chunk.
const RESERVATION_TTL: Duration = Duration::from_secs(24 * 60 * 60);

/// Number of shards in the per-user commit lock.
const LOCK_SHARDS: usize = 64;

/// A reservation would push the user past their quota. A dedicated type so the
/// outcome is self-describing at the call site.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct QuotaExceeded;

struct UploadReservation {
    bytes: i64,
    at: Instant,
}

struct BlockReservation {
    bytes: i64,
}

/// Block reservations for one `(user, repo)`, kept together so summing a user's
/// outstanding bytes costs one lookup per library rather than one per block.
struct RepoBlockReservations {
    total: i64,
    at: Instant,
    blocks: HashMap<String, BlockReservation>,
}

#[derive(Default)]
struct Reservations {
    uploads: HashMap<(i32, String, String), UploadReservation>,
    blocks: HashMap<(i32, String), RepoBlockReservations>,
}

impl Reservations {
    fn total_for(&self, user_id: i32) -> i64 {
        let uploads: i64 = self
            .uploads
            .iter()
            .filter(|((uid, _, _), _)| *uid == user_id)
            .map(|(_, r)| r.bytes)
            .sum();
        let blocks: i64 = self
            .blocks
            .iter()
            .filter(|((uid, _), _)| *uid == user_id)
            .map(|(_, r)| r.total)
            .sum();
        uploads + blocks
    }

    fn groups_for(&self, user_id: i32) -> usize {
        self.uploads
            .keys()
            .filter(|(uid, _, _)| *uid == user_id)
            .count()
            + self
                .blocks
                .keys()
                .filter(|(uid, _)| *uid == user_id)
                .count()
    }

    fn expire_before(&mut self, cutoff: Instant) -> usize {
        let before = self.uploads.len() + self.blocks.len();
        self.uploads.retain(|_, r| r.at >= cutoff);
        self.blocks.retain(|_, r| r.at >= cutoff);
        before - (self.uploads.len() + self.blocks.len())
    }
}

/// The per-user commit lock, held while a quota check and the commit that spends
/// the checked headroom are in flight. Owned so it can be moved into a service
/// helper without borrowing the cache for the whole request.
pub struct QuotaScope {
    user_id: i32,
    _guard: tokio::sync::OwnedMutexGuard<()>,
}

impl QuotaScope {
    pub fn user_id(&self) -> i32 {
        self.user_id
    }
}

pub struct QuotaCache {
    usage: Mutex<HashMap<i32, (i64, Instant)>>,
    reserved: Mutex<Reservations>,
    /// Sharded async mutexes serializing quota checks and commits per user. A
    /// collision between two users only costs some throughput, never correctness.
    locks: Vec<Arc<tokio::sync::Mutex<()>>>,
}

impl Default for QuotaCache {
    fn default() -> Self {
        Self::new()
    }
}

impl QuotaCache {
    pub fn new() -> Self {
        Self {
            usage: Mutex::new(HashMap::new()),
            reserved: Mutex::new(Reservations::default()),
            locks: (0..LOCK_SHARDS)
                .map(|_| Arc::new(tokio::sync::Mutex::new(())))
                .collect(),
        }
    }

    /// Serialize quota accounting for `user_id`.
    ///
    /// Callers must hold this scope across the quota check *and* the commit that
    /// spends the checked headroom; releasing it between the two reintroduces the
    /// check-then-commit race. The lock order is: quota scope, then repository
    /// lock, then the reservation-map lock. No path may take them in reverse.
    pub async fn lock_user(&self, user_id: i32) -> QuotaScope {
        let idx = (user_id as usize) % self.locks.len();
        let guard = self.locks[idx].clone().lock_owned().await;
        QuotaScope {
            user_id,
            _guard: guard,
        }
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

    /// Drop a user's snapshot so the next check re-reads the database.
    ///
    /// Commits must call this: otherwise the 500 ms snapshot stays in place and several
    /// uploads started in that window all see pre-commit usage, overshooting the quota
    /// by roughly the number of concurrent uploads.
    pub fn invalidate_usage(&self, user_id: i32) {
        let mut map = self.usage.lock().unwrap_or_else(|e| e.into_inner());
        map.remove(&user_id);
    }

    /// Bytes reserved by `user_id` across every repository.
    pub fn reserved_total(&self, user_id: i32) -> i64 {
        self.reserved
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .total_for(user_id)
    }

    /// Atomically reserve `delta` more bytes for one temp upload when
    /// `committed + reserved + delta <= quota`. Repeating the same upload id is a
    /// no-op, so a client that resends the first chunk does not reserve twice.
    pub fn reserve_upload(
        &self,
        user_id: i32,
        repo_id: &str,
        upload_id: &str,
        delta: i64,
        committed: i64,
        quota: i64,
    ) -> Result<(), QuotaExceeded> {
        if delta <= 0 {
            return Ok(());
        }
        let now = Instant::now();
        let mut reserved = self.reserved.lock().unwrap_or_else(|e| e.into_inner());
        reserved.expire_before(now - RESERVATION_TTL);

        let key = (user_id, repo_id.to_string(), upload_id.to_string());
        if reserved.uploads.contains_key(&key) {
            return Ok(());
        }
        if reserved.groups_for(user_id) >= RESERVED_MAX_PER_USER
            || reserved.uploads.len() + reserved.blocks.len() >= RESERVED_MAX
        {
            // Fail closed: a full reservation table means we cannot account for
            // this write. The alternative is writing blocks without any bound.
            return Err(QuotaExceeded);
        }
        if committed + reserved.total_for(user_id) + delta > quota {
            return Err(QuotaExceeded);
        }
        reserved.uploads.insert(
            key,
            UploadReservation {
                bytes: delta,
                at: now,
            },
        );
        Ok(())
    }

    /// Atomically reserve one block write. Repeating the same block id is a no-op,
    /// so a retried `PUT block` does not consume quota again. Many blocks of one
    /// large file share one `(user, repo)` group, so a file is not capped by the
    /// bookkeeping entry limit.
    pub fn reserve_block(
        &self,
        user_id: i32,
        repo_id: &str,
        block_id: &str,
        delta: i64,
        committed: i64,
        quota: i64,
    ) -> Result<(), QuotaExceeded> {
        if delta <= 0 {
            return Ok(());
        }
        let now = Instant::now();
        let mut reserved = self.reserved.lock().unwrap_or_else(|e| e.into_inner());
        reserved.expire_before(now - RESERVATION_TTL);

        let key = (user_id, repo_id.to_string());
        if reserved
            .blocks
            .get(&key)
            .is_some_and(|r| r.blocks.contains_key(block_id))
        {
            return Ok(());
        }
        let adds_group = !reserved.blocks.contains_key(&key);
        if adds_group
            && (reserved.groups_for(user_id) >= RESERVED_MAX_PER_USER
                || reserved.uploads.len() + reserved.blocks.len() >= RESERVED_MAX)
        {
            return Err(QuotaExceeded);
        }
        if committed + reserved.total_for(user_id) + delta > quota {
            return Err(QuotaExceeded);
        }

        let group = reserved
            .blocks
            .entry(key)
            .or_insert_with(|| RepoBlockReservations {
                total: 0,
                at: now,
                blocks: HashMap::new(),
            });
        group.total += delta;
        group.at = now;
        group
            .blocks
            .insert(block_id.to_string(), BlockReservation { bytes: delta });
        Ok(())
    }

    /// Drop one temp upload's reservation. Returns the bytes released.
    pub fn release_upload(&self, user_id: i32, repo_id: &str, upload_id: &str) -> i64 {
        let key = (user_id, repo_id.to_string(), upload_id.to_string());
        self.reserved
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .uploads
            .remove(&key)
            .map(|r| r.bytes)
            .unwrap_or(0)
    }

    /// Drop one block's reservation. Returns the bytes released.
    pub fn release_block(&self, user_id: i32, repo_id: &str, block_id: &str) -> i64 {
        self.release_blocks(user_id, repo_id, &[block_id.to_string()])
    }

    /// Drop several block reservations in one lock acquisition.
    pub fn release_blocks(&self, user_id: i32, repo_id: &str, block_ids: &[String]) -> i64 {
        let key = (user_id, repo_id.to_string());
        let mut reserved = self.reserved.lock().unwrap_or_else(|e| e.into_inner());
        let Some(group) = reserved.blocks.get_mut(&key) else {
            return 0;
        };
        let mut released = 0;
        for id in block_ids {
            if let Some(r) = group.blocks.remove(id) {
                released += r.bytes;
            }
        }
        group.total -= released;
        if group.blocks.is_empty() {
            reserved.blocks.remove(&key);
        }
        released
    }

    /// Drop every reservation for one `(user, repo)`. Used when a library is removed
    /// or its upload state is being discarded wholesale; commit paths must release
    /// only their own upload/block keys.
    pub fn release_repo(&self, user_id: i32, repo_id: &str) -> i64 {
        let mut reserved = self.reserved.lock().unwrap_or_else(|e| e.into_inner());
        let mut released = 0;

        let upload_keys: Vec<_> = reserved
            .uploads
            .keys()
            .filter(|(uid, rid, _)| *uid == user_id && rid == repo_id)
            .cloned()
            .collect();
        for key in upload_keys {
            if let Some(r) = reserved.uploads.remove(&key) {
                released += r.bytes;
            }
        }
        if let Some(group) = reserved.blocks.remove(&(user_id, repo_id.to_string())) {
            released += group.total;
        }
        released
    }

    /// Drop reservations past the fixed uncommitted-upload TTL.
    pub fn expire_stale(&self) -> usize {
        let cutoff = Instant::now() - RESERVATION_TTL;
        self.expire_before(cutoff)
    }

    /// Drop reservations older than `cutoff`. Returns how many groups were removed.
    pub fn expire_before(&self, cutoff: Instant) -> usize {
        self.reserved
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .expire_before(cutoff)
    }
}

#[cfg(test)]
mod tests {
    use super::{QuotaCache, QuotaExceeded};
    use std::time::{Duration, Instant};

    #[test]
    fn reservations_are_per_user_repo_and_kind() {
        let cache = QuotaCache::new();
        assert!(cache.reserve_upload(1, "r1", "u1", 10, 0, 1000).is_ok());
        assert!(cache.reserve_block(1, "r1", "b1", 5, 0, 1000).is_ok());
        assert!(cache.reserve_block(2, "r1", "b1", 7, 0, 1000).is_ok());

        assert_eq!(cache.reserved_total(1), 15);
        assert_eq!(cache.reserved_total(2), 7);

        assert_eq!(cache.release_upload(1, "r1", "u1"), 10);
        assert_eq!(cache.reserved_total(1), 5, "only the upload was released");
        assert_eq!(cache.reserved_total(2), 7, "other users untouched");
    }

    #[test]
    fn duplicate_reservations_are_idempotent() {
        let cache = QuotaCache::new();
        assert!(cache.reserve_block(1, "r1", "b1", 600, 0, 1000).is_ok());
        assert!(
            cache.reserve_block(1, "r1", "b1", 600, 0, 1000).is_ok(),
            "re-sending the same block must not reserve again"
        );
        assert_eq!(cache.reserved_total(1), 600);

        assert!(cache.reserve_upload(1, "r1", "u1", 400, 0, 1000).is_ok());
        assert!(
            cache.reserve_upload(1, "r1", "u1", 400, 0, 1000).is_ok(),
            "retrying the first chunk must not reserve again"
        );
        assert_eq!(cache.reserved_total(1), 1000);
    }

    #[test]
    fn reservation_is_refused_when_committed_plus_reserved_exceeds_quota() {
        let cache = QuotaCache::new();
        assert!(cache.reserve_block(1, "r1", "b1", 700, 900, 2000).is_ok());
        assert!(
            cache.reserve_block(1, "r1", "b2", 700, 900, 2000).is_err(),
            "the second reservation must not fit"
        );
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
        // The reason this is not a process-global static: two servers must not share state.
        let a = QuotaCache::new();
        let b = QuotaCache::new();
        a.store_usage(1, 100);
        assert!(a.reserve_block(1, "r1", "b1", 10, 0, 1000).is_ok());
        assert_eq!(b.cached_usage(1), None);
        assert_eq!(b.reserved_total(1), 0);
    }

    #[test]
    fn expired_reservations_are_dropped() {
        let cache = QuotaCache::new();
        assert!(cache.reserve_block(1, "r1", "b1", 10, 0, 1000).is_ok());
        let cutoff = Instant::now() + Duration::from_secs(1);
        assert_eq!(cache.expire_before(cutoff), 1);
        assert_eq!(cache.reserved_total(1), 0);
    }

    #[test]
    fn per_user_group_cap_fails_closed() {
        let cache = QuotaCache::new();
        for i in 0..super::RESERVED_MAX_PER_USER {
            cache
                .reserve_upload(1, "r1", &format!("u{i}"), 1, 0, i64::MAX)
                .expect("under the per-user cap");
        }
        assert_eq!(
            cache.reserve_upload(1, "r1", "overflow", 1, 0, i64::MAX),
            Err(QuotaExceeded),
            "a full per-user table must not silently allow the write"
        );
    }

    /// A large file has many blocks but one reservation group; the group cap must
    /// not turn the number of CDC blocks into an upload-size limit.
    #[test]
    fn many_blocks_in_one_repo_do_not_hit_the_group_cap() {
        let cache = QuotaCache::new();
        for i in 0..4096 {
            cache
                .reserve_block(1, "r1", &format!("b{i}"), 1, 0, i64::MAX)
                .expect("blocks share one group");
        }
        assert_eq!(cache.reserved_total(1), 4096);
        assert_eq!(cache.release_block(1, "r1", "b0"), 1);
        assert_eq!(cache.reserved_total(1), 4095);
    }

    #[tokio::test]
    async fn user_lock_serializes_scope_holders() {
        let cache = std::sync::Arc::new(QuotaCache::new());
        let first = cache.lock_user(7).await;
        let second = cache.clone();
        let task = tokio::spawn(async move {
            let _guard = second.lock_user(7).await;
            1
        });
        tokio::time::sleep(Duration::from_millis(20)).await;
        assert!(!task.is_finished(), "the second scope must wait");
        drop(first);
        assert_eq!(task.await.unwrap(), 1);
    }
}
