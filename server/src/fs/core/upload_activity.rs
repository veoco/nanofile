//! Process-local markers of recent library writes.
//!
//! GC must not prune a library while a client may still be uploading into it. A
//! sync upload spans several requests (commit object, FS packs, blocks, then the
//! branch update), so a lock held for one request cannot cover the sequence.
//! Instead, every write entry point touches a per-library timestamp *before* its
//! first database write; a GC pass skips any library touched within
//! [`UPLOAD_ACTIVITY_TTL`]. The window is deliberately generous: an upload idle
//! for longer is treated like a block older than `gc.min_block_age_secs` —
//! abandoned, and eligible for a later pass.

use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant};

/// How long a library stays protected after its last write. The lease is
/// refreshed by every write, including each block upload and each FS pack.
pub(crate) const UPLOAD_ACTIVITY_TTL: Duration = Duration::from_secs(3600);

static ACTIVITY: LazyLock<Mutex<HashMap<String, Instant>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

fn lock() -> std::sync::MutexGuard<'static, HashMap<String, Instant>> {
    ACTIVITY
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Record a write against `repo_id`.
///
/// Call this before the operation's first database write: a GC pass that starts
/// in between must still see the activity and leave the repository alone.
pub(crate) fn touch(repo_id: &str) {
    lock().insert(repo_id.to_string(), Instant::now());
}

/// Whether `repo_id` was written within `ttl`.
///
/// `ttl == 0` disables the check; `GcPolicy::immediate()` uses that to keep the
/// pre-lease reachability semantics for tests.
pub(crate) fn is_active(repo_id: &str, ttl: Duration) -> bool {
    if ttl.is_zero() {
        return false;
    }
    lock().get(repo_id).is_some_and(|last| last.elapsed() < ttl)
}

#[cfg(test)]
pub(crate) fn clear(repo_id: &str) {
    lock().remove(repo_id);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn touch_marks_only_the_touched_repo() {
        let repo = "upload-activity-test-a";
        let other = "upload-activity-test-b";
        touch(repo);
        assert!(is_active(repo, UPLOAD_ACTIVITY_TTL));
        assert!(!is_active(other, UPLOAD_ACTIVITY_TTL));
        assert!(!is_active(repo, Duration::ZERO));
        clear(repo);
        assert!(!is_active(repo, UPLOAD_ACTIVITY_TTL));
    }

    #[test]
    fn a_repo_without_a_touch_is_inactive() {
        assert!(!is_active(
            "upload-activity-test-never-touched",
            UPLOAD_ACTIVITY_TTL
        ));
    }
}
