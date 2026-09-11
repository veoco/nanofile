//! Bounded traversal guards for directory-tree walks.
//!
//! Every FS walk in this crate is a level-by-level BFS over `dirents`. Because
//! `recv-fs` trusts the object id a client supplies for the object bytes
//! (verified separately in `SyncService::insert_fs_objects`), a client can
//! still craft a directory object that references itself, which would make an
//! unbounded walk expand forever. These guards bound the work:
//!
//! * [`TreeGuard::enter_level`] caps the BFS depth (a cycle always increases
//!   depth, so it terminates),
//! * [`TreeGuard::visit`] caps the total number of visited nodes.
//!
//! A **global** visited-set would be wrong here: Seafile content-addresses
//! directory objects, so the same dir id legitimately appears under several
//! parents, and de-duplicating would change size/quota semantics.
//!
//! Limits are process-wide and configured from `[sync]` at startup; the
//! defaults are far above any real tree.

use std::sync::atomic::{AtomicUsize, Ordering};

use base::error::AppError;

/// Default maximum BFS depth. Real trees are tens of levels at most.
pub const DEFAULT_MAX_TREE_DEPTH: usize = 512;
/// Default maximum number of visited directory nodes per walk.
pub const DEFAULT_MAX_TREE_VISITS: usize = 1_000_000;

static MAX_TREE_DEPTH: AtomicUsize = AtomicUsize::new(DEFAULT_MAX_TREE_DEPTH);
static MAX_TREE_VISITS: AtomicUsize = AtomicUsize::new(DEFAULT_MAX_TREE_VISITS);

/// Configure the traversal limits (called once at startup from `[sync]`).
pub fn configure(max_depth: usize, max_visits: usize) {
    MAX_TREE_DEPTH.store(max_depth.max(1), Ordering::Relaxed);
    MAX_TREE_VISITS.store(max_visits.max(1), Ordering::Relaxed);
}

/// Current maximum BFS depth.
pub fn max_tree_depth() -> usize {
    MAX_TREE_DEPTH.load(Ordering::Relaxed)
}

/// Current maximum visited-node count.
pub fn max_tree_visits() -> usize {
    MAX_TREE_VISITS.load(Ordering::Relaxed)
}

/// Depth/visit counter for one BFS walk.
///
/// Create one per walk, call [`TreeGuard::enter_level`] at the top of each
/// `while !frontier.is_empty()` iteration and [`TreeGuard::visit`] with the
/// number of nodes processed at that level.
#[derive(Debug, Default)]
pub struct TreeGuard {
    depth: usize,
    visits: usize,
}

impl TreeGuard {
    pub fn new() -> Self {
        Self::default()
    }

    /// Enter the next BFS level, failing once the depth cap is exceeded.
    pub fn enter_level(&mut self) -> Result<(), AppError> {
        self.depth += 1;
        if self.depth > max_tree_depth() {
            return Err(AppError::BadRequest(format!(
                "directory tree traversal exceeded the maximum depth ({})",
                max_tree_depth()
            )));
        }
        Ok(())
    }

    /// Account for `count` visited nodes, failing once the cap is exceeded.
    pub fn visit(&mut self, count: usize) -> Result<(), AppError> {
        self.visits = self.visits.saturating_add(count);
        if self.visits > max_tree_visits() {
            return Err(AppError::BadRequest(format!(
                "directory tree traversal exceeded the maximum node count ({})",
                max_tree_visits()
            )));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// Both tests below rewrite the process-wide limits via [`configure`], and
    /// `cargo test` runs them concurrently, so without this lock one test's
    /// `configure` can land between the other's setup and assertion (it would
    /// restore the 1,000,000-visit default and make `visit(1)` succeed).
    static LIMITS_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn depth_cap_terminates_a_cycle() {
        let _guard = LIMITS_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        configure(4, 1000);
        let mut guard = TreeGuard::new();
        for _ in 0..4 {
            assert!(guard.enter_level().is_ok());
        }
        assert!(guard.enter_level().is_err());
        // Restore defaults for other tests in the same process.
        configure(DEFAULT_MAX_TREE_DEPTH, DEFAULT_MAX_TREE_VISITS);
    }

    #[test]
    fn visit_cap_terminates_wide_trees() {
        let _guard = LIMITS_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        configure(100, 10);
        let mut guard = TreeGuard::new();
        assert!(guard.visit(10).is_ok());
        assert!(guard.visit(1).is_err());
        configure(DEFAULT_MAX_TREE_DEPTH, DEFAULT_MAX_TREE_VISITS);
    }
}
