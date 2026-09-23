//! The bounded in-memory table of job runs.
//!
//! One store replaces the four hand-rolled maps the task code used to keep
//! (copy/move tasks, the reindex progress map plus its dedup twin, and the zip
//! token registry). Retention is enforced in one place, by one sweeper, so no
//! read path has to garbage-collect while holding a lock.
//!
//! Two budgets, not one: a count cap alone does not bound memory when a single
//! run carries a batch of a thousand file names, and a byte cap alone does not
//! bound the cost of walking the table.

use std::collections::HashMap;
use std::sync::atomic::{AtomicI64, AtomicUsize, Ordering};
use std::sync::{Arc, PoisonError, RwLock};

use base::error::AppError;

use super::run::{JobRun, JobState, RunId, Viewer};
use super::spec::JobKey;

/// How much the store may hold.
#[derive(Clone, Copy, Debug)]
pub struct RunLimits {
    /// Maximum number of runs (active runs are never evicted).
    pub max_retained: usize,
    /// Maximum total footprint in bytes, as measured by [`JobRun::bytes`].
    pub max_retained_bytes: usize,
    /// How long a terminal run stays readable.
    pub terminal_ttl_secs: i64,
}

impl Default for RunLimits {
    fn default() -> Self {
        Self {
            max_retained: 1000,
            max_retained_bytes: 8 * 1024 * 1024,
            terminal_ttl_secs: 3600,
        }
    }
}

/// Which runs a caller wants to see.
#[derive(Clone, Copy, Debug, Default)]
pub struct RunFilter {
    pub key: Option<JobKey>,
    pub owner: Option<i32>,
    /// Include runs that are still active.
    pub include_active: bool,
    /// Include runs that have finished.
    pub include_terminal: bool,
    /// Newest first, at most this many. `0` means no limit.
    pub limit: usize,
}

impl RunFilter {
    /// Everything, newest first.
    pub fn all() -> Self {
        Self {
            include_active: true,
            include_terminal: true,
            ..Default::default()
        }
    }

    fn matches(&self, run: &JobRun) -> bool {
        if let Some(key) = self.key
            && run.key != key
        {
            return false;
        }
        if let Some(owner) = self.owner
            && run.owner != Some(owner)
        {
            return false;
        }
        if run.state.is_terminal() {
            self.include_terminal
        } else {
            self.include_active
        }
    }
}

struct Inner {
    runs: RwLock<HashMap<RunId, JobRun>>,
    /// Maintained alongside `runs` so the byte check is O(1).
    bytes: AtomicUsize,
    /// Budgets are atomics so a live settings change needs no lock and cannot
    /// race the reader.
    max_retained: AtomicUsize,
    max_retained_bytes: AtomicUsize,
    terminal_ttl_secs: AtomicI64,
}

/// A shared, bounded table of job runs.
#[derive(Clone)]
pub struct RunStore {
    inner: Arc<Inner>,
}

impl std::fmt::Debug for RunStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RunStore")
            .field("len", &self.len())
            .field("bytes", &self.bytes())
            .finish()
    }
}

impl RunStore {
    pub fn new(limits: RunLimits) -> Self {
        Self {
            inner: Arc::new(Inner {
                runs: RwLock::new(HashMap::new()),
                bytes: AtomicUsize::new(0),
                max_retained: AtomicUsize::new(limits.max_retained),
                max_retained_bytes: AtomicUsize::new(limits.max_retained_bytes),
                terminal_ttl_secs: AtomicI64::new(limits.terminal_ttl_secs),
            }),
        }
    }

    fn read(&self) -> std::sync::RwLockReadGuard<'_, HashMap<RunId, JobRun>> {
        self.inner
            .runs
            .read()
            .unwrap_or_else(PoisonError::into_inner)
    }

    fn write(&self) -> std::sync::RwLockWriteGuard<'_, HashMap<RunId, JobRun>> {
        self.inner
            .runs
            .write()
            .unwrap_or_else(PoisonError::into_inner)
    }

    pub fn len(&self) -> usize {
        self.read().len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn bytes(&self) -> usize {
        self.inner.bytes.load(Ordering::Relaxed)
    }

    pub fn limits(&self) -> RunLimits {
        RunLimits {
            max_retained: self.inner.max_retained.load(Ordering::Relaxed),
            max_retained_bytes: self.inner.max_retained_bytes.load(Ordering::Relaxed),
            terminal_ttl_secs: self.inner.terminal_ttl_secs.load(Ordering::Relaxed),
        }
    }

    /// Replace the budgets on a running server; the next insert enforces them.
    pub fn set_limits(&self, limits: RunLimits) {
        self.inner
            .max_retained
            .store(limits.max_retained, Ordering::Relaxed);
        self.inner
            .max_retained_bytes
            .store(limits.max_retained_bytes, Ordering::Relaxed);
        self.inner
            .terminal_ttl_secs
            .store(limits.terminal_ttl_secs, Ordering::Relaxed);
    }

    /// Store a run, evicting terminal runs oldest-first until both budgets
    /// hold.
    ///
    /// Returns [`AppError::TooManyRequests`] when this single run exceeds the
    /// byte budget: it can never be stored, so it is a refusal rather than
    /// something to wait for.
    pub fn insert(&self, run: JobRun) -> Result<(), AppError> {
        let size = run.bytes();
        let limits = self.limits();
        if limits.max_retained_bytes > 0 && size > limits.max_retained_bytes {
            return Err(AppError::TooManyRequests);
        }

        let mut runs = self.write();
        self.evict_until_room(&mut runs, size);
        runs.insert(run.id.clone(), run);
        self.inner.bytes.fetch_add(size, Ordering::Relaxed);
        Ok(())
    }

    /// Apply `f` to a run. Returns `false` when the run is gone.
    pub fn update(&self, id: &RunId, f: impl FnOnce(&mut JobRun)) -> bool {
        let mut runs = self.write();
        let Some(run) = runs.get_mut(id) else {
            return false;
        };
        let before = run.bytes();
        f(run);
        let after = run.bytes();
        if after >= before {
            self.inner
                .bytes
                .fetch_add(after - before, Ordering::Relaxed);
        } else {
            self.inner
                .bytes
                .fetch_sub(before - after, Ordering::Relaxed);
        }
        true
    }

    pub fn get(&self, id: &RunId) -> Option<JobRun> {
        self.read().get(id).cloned()
    }

    /// Fetch a run the viewer is allowed to see; anything else reads as absent.
    pub fn get_for(&self, id: &RunId, viewer: Viewer) -> Option<JobRun> {
        self.get(id).filter(|run| run.is_visible_to(viewer))
    }

    /// Runs matching `filter`, newest first.
    pub fn list(&self, filter: &RunFilter) -> Vec<JobRun> {
        let mut out: Vec<JobRun> = self
            .read()
            .values()
            .filter(|run| filter.matches(run))
            .cloned()
            .collect();
        out.sort_unstable_by(|a, b| {
            b.created_at
                .cmp(&a.created_at)
                .then_with(|| b.id.cmp(&a.id))
        });
        if filter.limit > 0 {
            out.truncate(filter.limit);
        }
        out
    }

    /// The first active run of `key` whose `params[field]` equals `value`.
    ///
    /// Used for [`super::spec::Dedup::ByParams`], which replaces the per-repo
    /// reindex lock.
    pub fn find_active_by_param(
        &self,
        key: JobKey,
        field: &str,
        value: &serde_json::Value,
    ) -> Option<JobRun> {
        self.read()
            .values()
            .find(|run| {
                run.key == key
                    && run.state.is_active()
                    && run
                        .params
                        .as_ref()
                        .and_then(|p| p.get(field))
                        .is_some_and(|v| v == value)
            })
            .cloned()
    }

    /// How many active runs exist for `key`, optionally limited to one owner.
    pub fn count_active(&self, key: JobKey, owner: Option<i32>) -> usize {
        self.read()
            .values()
            .filter(|run| {
                run.key == key
                    && run.state.is_active()
                    && owner.is_none_or(|o| run.owner == Some(o))
            })
            .count()
    }

    /// How many runs are active in total, optionally for one owner.
    pub fn count_active_all(&self, owner: Option<i32>) -> usize {
        self.read()
            .values()
            .filter(|run| run.state.is_active() && owner.is_none_or(|o| run.owner == Some(o)))
            .count()
    }

    /// Drop expired terminal runs and reclaim their bytes. Returns how many
    /// were removed.
    ///
    /// Called by one periodic sweeper and on insert; never from a read path.
    pub fn sweep(&self, now: i64) -> usize {
        let ttl = self.limits().terminal_ttl_secs;
        let mut runs = self.write();
        let mut dropped = 0usize;
        let mut freed = 0usize;
        runs.retain(|_, run| {
            let expired = run.state.is_terminal()
                && ttl > 0
                && now.saturating_sub(run.finished_at.unwrap_or(run.created_at)) >= ttl;
            if expired {
                dropped += 1;
                freed += run.bytes();
            }
            !expired
        });
        if freed > 0 {
            self.inner.bytes.fetch_sub(freed, Ordering::Relaxed);
        }
        dropped
    }

    /// Mark every active run as [`JobState::Interrupted`].
    ///
    /// Called at a generation boundary: the process keeps running but the
    /// generation's handlers are gone, so a run that has not finished will
    /// never report again. Recording it is what stops a client polling a
    /// `task_id` from getting a 404 after an administrator restarts the server.
    pub fn interrupt_active(&self, now: i64) -> usize {
        let mut runs = self.write();
        let mut count = 0usize;
        let mut freed = 0usize;
        for run in runs.values_mut() {
            if run.state.is_active() {
                run.state = JobState::Interrupted;
                run.finished_at = Some(now);
                let before = run.bytes();
                run.drop_params();
                freed += before - run.bytes();
                count += 1;
            }
        }
        if freed > 0 {
            self.inner.bytes.fetch_sub(freed, Ordering::Relaxed);
        }
        count
    }

    /// Evict terminal runs, oldest first, until `incoming` more bytes fit.
    fn evict_until_room(&self, runs: &mut HashMap<RunId, JobRun>, incoming: usize) {
        let limits = self.limits();
        loop {
            let over_count = limits.max_retained > 0 && runs.len() >= limits.max_retained;
            let over_bytes = limits.max_retained_bytes > 0
                && self.inner.bytes.load(Ordering::Relaxed) + incoming > limits.max_retained_bytes;
            if !over_count && !over_bytes {
                return;
            }
            let Some(oldest) = runs
                .values()
                .filter(|run| run.state.is_terminal())
                .min_by_key(|run| (run.finished_at.unwrap_or(run.created_at), run.id.clone()))
                .map(|run| run.id.clone())
            else {
                // Everything left is active: the budget cannot be met without
                // discarding live work, so let the table grow past it. Active
                // runs are bounded by the admission caps instead.
                return;
            };
            if let Some(removed) = runs.remove(&oldest) {
                self.inner
                    .bytes
                    .fetch_sub(removed.bytes(), Ordering::Relaxed);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tasks::run::{JobRun, Progress};
    use crate::tasks::spec::Visibility;

    fn run_at(owner: i32, created_at: i64) -> JobRun {
        JobRun::queued(
            JobKey::Copy,
            Visibility::Owner,
            Some(owner),
            serde_json::json!({"src_dirents": ["a"]}),
            "Copy 1 item",
            Some(1),
            created_at,
        )
    }

    fn finish(run: JobRun, at: i64) -> JobRun {
        let mut run = run;
        run.state = JobState::Succeeded;
        run.finished_at = Some(at);
        run.drop_params();
        run
    }

    fn store(max_retained: usize, max_bytes: usize, ttl: i64) -> RunStore {
        RunStore::new(RunLimits {
            max_retained,
            max_retained_bytes: max_bytes,
            terminal_ttl_secs: ttl,
        })
    }

    #[test]
    fn insert_and_fetch_round_trip() {
        let s = store(10, 0, 3600);
        let run = run_at(1, 100);
        let id = run.id.clone();
        s.insert(run).unwrap();
        assert_eq!(s.len(), 1);
        assert!(s.bytes() > 0);
        assert_eq!(s.get(&id).unwrap().owner, Some(1));
        assert!(s.get(&RunId::from_client("nope")).is_none());
    }

    #[test]
    fn visibility_filters_the_owner_lookup() {
        let s = store(10, 0, 3600);
        let run = run_at(7, 100);
        let id = run.id.clone();
        s.insert(run).unwrap();
        assert!(s.get_for(&id, Viewer::user(7)).is_some());
        assert!(
            s.get_for(&id, Viewer::user(8)).is_none(),
            "another user must not see the run"
        );
        // Internal callers still get the raw record.
        assert!(s.get(&id).is_some());
    }

    #[test]
    fn byte_budget_evicts_the_oldest_terminal_run() {
        // Room for exactly two of these runs.
        let probe = run_at(1, 0).bytes();
        let s = store(0, probe * 2, 3600);

        let first = run_at(1, 100);
        let first_id = first.id.clone();
        let second = run_at(1, 200);
        let second_id = second.id.clone();
        s.insert(finish(first, 100)).unwrap();
        s.insert(finish(second, 200)).unwrap();
        let newest = run_at(1, 300);
        let newest_id = newest.id.clone();
        // Active, so it can never be the one evicted.
        s.insert(newest).unwrap();

        assert_eq!(s.len(), 2, "the byte budget holds two runs");
        assert!(s.get(&newest_id).is_some(), "the active run survives");
        assert!(
            !s.get(&first_id).is_some() || !s.get(&second_id).is_some(),
            "at least one terminal run was evicted"
        );
        assert!(s.bytes() <= probe * 2, "the tally stays within budget");
    }

    #[test]
    fn active_runs_are_never_evicted() {
        let probe = run_at(1, 0).bytes();
        let s = store(1, 0, 3600);
        // max_retained = 1, but both are active: the table grows rather than
        // discarding live work.
        s.insert(run_at(1, 100)).unwrap();
        s.insert(run_at(1, 200)).unwrap();
        assert_eq!(s.len(), 2);
        assert_eq!(s.count_active_all(None), 2);
        assert!(s.bytes() >= probe * 2);
    }

    #[test]
    fn an_oversized_run_is_refused() {
        let s = store(10, 1, 3600);
        assert!(matches!(
            s.insert(run_at(1, 100)),
            Err(AppError::TooManyRequests)
        ));
        assert!(s.is_empty());
    }

    #[test]
    fn sweep_drops_expired_terminal_runs_and_frees_bytes() {
        let s = store(10, 0, 100);
        s.insert(finish(run_at(1, 100), 100)).unwrap();
        s.insert(finish(run_at(1, 250), 250)).unwrap();
        s.insert(run_at(1, 260)).unwrap();
        let before = s.bytes();

        assert_eq!(s.sweep(300), 1, "only the run older than the TTL goes");
        assert_eq!(s.len(), 2);
        assert!(s.bytes() < before);
    }

    /// A run that never started is swept by its creation time, so a crashed
    /// generation cannot leave queued entries forever.
    #[test]
    fn sweep_uses_created_at_when_a_run_never_finished() {
        let s = store(10, 0, 100);
        let mut run = run_at(1, 100);
        run.state = JobState::Failed("boom".into());
        s.insert(run).unwrap();
        assert_eq!(s.sweep(201), 1);
        assert!(s.is_empty());
    }

    #[test]
    fn interrupt_marks_every_active_run() {
        let s = store(10, 0, 3600);
        let active = run_at(1, 100);
        let active_id = active.id.clone();
        let mut done = run_at(1, 100);
        done.state = JobState::Succeeded;
        let done_id = done.id.clone();
        s.insert(active).unwrap();
        s.insert(done).unwrap();

        assert_eq!(s.interrupt_active(500), 1);
        let run = s.get(&active_id).unwrap();
        assert_eq!(run.state, JobState::Interrupted);
        assert_eq!(run.finished_at, Some(500));
        assert!(
            run.params.is_none(),
            "params are released on the transition"
        );
        assert_eq!(
            s.get(&done_id).unwrap().state,
            JobState::Succeeded,
            "a finished run is left alone"
        );
    }

    #[test]
    fn list_filters_by_key_owner_and_state() {
        let s = store(10, 0, 3600);
        s.insert(run_at(1, 100)).unwrap();
        s.insert(run_at(2, 200)).unwrap();
        let mut reindex = run_at(1, 300);
        reindex.key = JobKey::Reindex;
        s.insert(reindex).unwrap();

        let all = s.list(&RunFilter::all());
        assert_eq!(all.len(), 3);
        assert_eq!(all[0].created_at, 300, "newest first");

        let mine = s.list(&RunFilter {
            owner: Some(1),
            ..RunFilter::all()
        });
        assert_eq!(mine.len(), 2);

        let only_reindex = s.list(&RunFilter {
            key: Some(JobKey::Reindex),
            ..RunFilter::all()
        });
        assert_eq!(only_reindex.len(), 1);

        let terminal_only = s.list(&RunFilter {
            include_active: false,
            include_terminal: true,
            ..Default::default()
        });
        assert_eq!(terminal_only.len(), 0);

        assert_eq!(
            s.list(&RunFilter {
                limit: 2,
                ..RunFilter::all()
            })
            .len(),
            2
        );
    }

    #[test]
    fn dedup_finds_an_active_run_by_a_param() {
        let s = store(10, 0, 3600);
        let mut run = run_at(1, 100);
        run.key = JobKey::Reindex;
        run.params = Some(serde_json::json!({"repo_id": "r1"}));
        let id = run.id.clone();
        s.insert(run).unwrap();

        assert!(
            s.find_active_by_param(JobKey::Reindex, "repo_id", &serde_json::json!("r1"))
                .is_some()
        );
        assert!(
            s.find_active_by_param(JobKey::Reindex, "repo_id", &serde_json::json!("r2"))
                .is_none()
        );
        assert!(
            s.find_active_by_param(JobKey::Copy, "repo_id", &serde_json::json!("r1"))
                .is_none(),
            "a different job key does not match"
        );

        // A terminal run no longer blocks a fresh submission.
        s.update(&id, |r| {
            r.state = JobState::Succeeded;
            r.drop_params();
        });
        assert!(
            s.find_active_by_param(JobKey::Reindex, "repo_id", &serde_json::json!("r1"))
                .is_none()
        );
    }

    #[test]
    fn count_active_can_be_scoped_to_an_owner() {
        let s = store(10, 0, 3600);
        s.insert(run_at(1, 100)).unwrap();
        s.insert(run_at(1, 200)).unwrap();
        s.insert(run_at(2, 300)).unwrap();
        assert_eq!(s.count_active(JobKey::Copy, None), 3);
        assert_eq!(s.count_active(JobKey::Copy, Some(1)), 2);
        assert_eq!(s.count_active(JobKey::Copy, Some(9)), 0);
        assert_eq!(s.count_active(JobKey::Reindex, None), 0);
    }

    #[test]
    fn update_adjusts_the_byte_tally() {
        let s = store(10, 0, 3600);
        let run = run_at(1, 100);
        let id = run.id.clone();
        s.insert(run).unwrap();
        let before = s.bytes();

        s.update(&id, |r| {
            r.progress = Progress {
                done: 1,
                total: Some(1),
                message: "a much longer progress message".to_string(),
            };
        });
        assert!(s.bytes() > before, "a bigger run raises the tally");

        assert!(!s.update(&RunId::from_client("missing"), |_| {}));
    }

    #[test]
    fn set_limits_applies_to_the_next_insert() {
        let s = store(0, 0, 3600);
        s.insert(run_at(1, 100)).unwrap();
        let probe = run_at(1, 0).bytes();
        s.set_limits(RunLimits {
            max_retained: 0,
            max_retained_bytes: probe,
            terminal_ttl_secs: 3600,
        });
        // Active runs are never evicted, so the insert still succeeds.
        assert!(s.insert(run_at(1, 200)).is_ok());
        assert_eq!(s.limits().max_retained_bytes, probe);
    }
}
