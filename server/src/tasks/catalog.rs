//! The one place a job's tunables are written down.
//!
//! Every policy decision about every job — interval, concurrency, retention,
//! deferral, crash replay — lives in [`policy`] below. A policy is
//! process-lifetime and static, while the bodies in `tasks::setup` capture the
//! current server generation's resources. An interval is a reaction time, not a
//! schedule; a pass that usually finds nothing is declared with `notable`, which
//! keeps its idle ticks out of the run list — see [`History`] for the rule.

use super::spec::{
    ChunkPolicy, Dedup, Durability, History, JobKey, JobSpec, OverlapPolicy, Priority, QuietPolicy,
    Resource, RetryPolicy, SpikePolicy, TimeoutPolicy, Trigger, Visibility,
};

fn fixed_timer(interval_secs: u64) -> Trigger {
    Trigger::Periodic {
        interval_secs,
        overlap: OverlapPolicy::Skip,
    }
}

/// Marks a pass that fires far more often than it has work: its idle ticks are
/// not history, so it must not bury the runs that did something. Its lifetime
/// counters on the registry page still show every tick.
fn notable(mut spec: JobSpec) -> JobSpec {
    spec.history = History::Notable;
    spec
}

/// The policy for `key`.
pub fn policy(key: JobKey) -> JobSpec {
    match key {
        JobKey::Copy => client_job(key, "copy"),
        JobKey::Move => client_job(key, "move"),
        JobKey::Reindex => JobSpec {
            name: "reindex",
            priority: Priority::Normal,
            resource: Resource::Cpu,
            max_concurrent: 4,
            timeout: TimeoutPolicy {
                no_progress_for_secs: Some(300),
                max_total_secs: None,
            },
            dedup: Dedup::ByParams("repo_id"),
            visibility: Visibility::OwnerOrAdmin,
            // Idempotent — a full rebuild converges on the same index — so this
            // is the one job safe to replay after a crash.
            durability: Durability::Durable,
            resumable: true,
            cancellable: true,
            chunkable: Some(ChunkPolicy {
                max_chunk_ms: 500,
                unit: "file",
            }),
            force_safe: true,
            idempotent: true,
            ..client_job(key, "reindex")
        },
        // Indexing the files a mutation produced, one bounded batch per run, so
        // a mass sync is a handful of runs rather than thousands of active ones.
        // Nobody polls this run, and redoing a file is idempotent, which is what
        // makes it safely cancellable.
        JobKey::IndexFiles => JobSpec {
            key,
            name: "index files",
            trigger: Trigger::OnDemand,
            priority: Priority::Normal,
            resource: Resource::Cpu,
            max_concurrent: 4,
            queue_depth: 0,
            timeout: TimeoutPolicy {
                no_progress_for_secs: Some(300),
                max_total_secs: None,
            },
            retry: RetryPolicy::Never,
            dedup: Dedup::None,
            visibility: Visibility::OwnerOrAdmin,
            durability: Durability::Memory,
            history: History::EveryRun,
            resumable: false,
            // A batch is a sequence of independent files, so stopping between
            // them loses nothing the backfill will not rediscover.
            cancellable: true,
            chunkable: Some(ChunkPolicy {
                max_chunk_ms: 500,
                unit: "file",
            }),
            force_safe: false,
            // An upload is waiting for the file to be searchable, so this is
            // never deferred; the backfill is the pass that waits for idle.
            quiet: None,
            idempotent: true,
        },

        JobKey::TokenExpiryCheck => {
            notable(housekeeping(key, "token expiry check", fixed_timer(3600)))
        }
        JobKey::PasswordCacheCleanup => notable(housekeeping(
            key,
            "password cache cleanup",
            fixed_timer(900),
        )),
        JobKey::ExpiredDataCleanup => {
            notable(housekeeping(key, "expired data cleanup", fixed_timer(3600)))
        }
        // A backstop, not the commit path: the indexer commits a debounced
        // write within 100ms on its own, so this pass only has work when that
        // commit failed, and two minutes is short enough that such a failure
        // cannot leave much uncommitted.
        JobKey::IndexCommit => notable(housekeeping(key, "index commit", fixed_timer(120))),
        // The low-load pass that catches up with pre-existing files and with
        // files an extractor gained support for since they were last seen.
        // Yields rather than throttles: a half-done pass is resumable from its
        // cursor, and the checkpoint is a file boundary.
        JobKey::IndexBackfill => JobSpec {
            key,
            name: "index backfill",
            trigger: fixed_timer(300),
            priority: Priority::Background,
            resource: Resource::Cpu,
            max_concurrent: 1,
            queue_depth: 0,
            timeout: TimeoutPolicy {
                no_progress_for_secs: Some(1800),
                max_total_secs: None,
            },
            retry: RetryPolicy::Never,
            dedup: Dedup::None,
            visibility: Visibility::OwnerOrAdmin,
            // Audited rather than durable: replaying a pass after a crash buys
            // nothing the next tick does not do.
            durability: Durability::Audit,
            history: History::Notable,
            resumable: true,
            cancellable: false,
            chunkable: Some(ChunkPolicy {
                max_chunk_ms: 500,
                unit: "file",
            }),
            force_safe: false,
            quiet: Some(QuietPolicy {
                min_idle_for_secs: 30,
                max_deferral_hours: 0,
                on_spike: SpikePolicy::Yield,
            }),
            idempotent: true,
        },
        JobKey::ZipTaskCleanup => notable(housekeeping(key, "zip task cleanup", fixed_timer(600))),

        JobKey::GarbageCollection => JobSpec {
            name: "gc",
            // The interval comes from `[gc] interval_hours`; `tasks::setup`
            // replaces the trigger at registration.
            trigger: Trigger::Manual,
            priority: Priority::Background,
            resource: Resource::BlockIo,
            timeout: TimeoutPolicy {
                // A large library takes as long as it takes. What must not
                // happen is a pass that stops getting anywhere.
                no_progress_for_secs: Some(1800),
                max_total_secs: None,
            },
            // Deferrable because a pass can be interrupted at a repository
            // boundary. `force_safe` is false: GC racing an in-flight upload
            // deletes blocks a commit is about to reference, so a deferral that
            // drags on must be reported rather than forced.
            chunkable: Some(ChunkPolicy {
                max_chunk_ms: 500,
                unit: "repository",
            }),
            quiet: Some(QuietPolicy {
                min_idle_for_secs: 60,
                max_deferral_hours: 24,
                on_spike: SpikePolicy::Yield,
            }),
            ..housekeeping(key, "gc", Trigger::Manual)
        },
        JobKey::BlockEncryptionConvert => JobSpec {
            name: "block encryption convert",
            priority: Priority::Background,
            resource: Resource::BlockIo,
            timeout: TimeoutPolicy {
                no_progress_for_secs: Some(1800),
                max_total_secs: None,
            },
            chunkable: Some(ChunkPolicy {
                max_chunk_ms: 500,
                unit: "batch",
            }),
            // Optional work: it may wait indefinitely, so the deferral cap is
            // zero — never force it.
            quiet: Some(QuietPolicy {
                min_idle_for_secs: 30,
                max_deferral_hours: 0,
                on_spike: SpikePolicy::Throttle,
            }),
            ..housekeeping(key, "block encryption convert", Trigger::Manual)
        },
    }
}

/// The policy for every key, in [`JobKey::ALL`] order.
pub fn all() -> Vec<JobSpec> {
    JobKey::ALL.iter().copied().map(policy).collect()
}

/// A job whose run is submitted by a request and polled by the same client.
///
/// Copy and move are reference operations — no blocks are touched, one tree
/// update plus one commit — and are jobs only because the desktop client polls
/// a task id.
fn client_job(key: JobKey, name: &'static str) -> JobSpec {
    JobSpec {
        key,
        name,
        trigger: Trigger::OnDemand,
        priority: Priority::Interactive,
        resource: Resource::DbWrite,
        max_concurrent: 8,
        queue_depth: 0,
        // One tree mutation plus a commit: a wall-clock cap is enough.
        timeout: TimeoutPolicy {
            no_progress_for_secs: None,
            max_total_secs: Some(300),
        },
        // Not idempotent: a conflicting destination name is resolved by
        // generating a unique one, so a rerun would leave a duplicate behind.
        retry: RetryPolicy::Never,
        dedup: Dedup::None,
        // Owner-only with no administrator override: everyone else gets a 404
        // from the copy/move progress endpoint.
        visibility: Visibility::Owner,
        durability: Durability::Memory,
        history: History::EveryRun,
        // A move is two commits, and interrupting between them loses the entry
        // from HEAD; a copy has nothing to resume. Neither is cancellable.
        resumable: false,
        cancellable: false,
        chunkable: None,
        force_safe: false,
        quiet: None,
        idempotent: false,
    }
}

/// A periodic pass whose lateness has no cost and whose rerun is harmless.
fn housekeeping(key: JobKey, name: &'static str, trigger: Trigger) -> JobSpec {
    JobSpec {
        key,
        name,
        trigger,
        priority: Priority::Normal,
        resource: Resource::Cpu,
        max_concurrent: 1,
        queue_depth: 0,
        timeout: TimeoutPolicy {
            no_progress_for_secs: None,
            max_total_secs: None,
        },
        retry: RetryPolicy::Never,
        dedup: Dedup::None,
        visibility: Visibility::OwnerOrAdmin,
        // Audited rather than durable: replaying a cleanup pass buys nothing,
        // though recording it is useful history.
        durability: Durability::Audit,
        history: History::EveryRun,
        resumable: true,
        cancellable: false,
        chunkable: None,
        force_safe: false,
        // Latency-sensitive jobs must never be gated on idle; the rest are
        // cheap enough that gating would not pay for itself.
        quiet: None,
        idempotent: true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every key must have a policy that satisfies the registry's invariants, so
    /// a mis-declared job fails `cargo test` rather than server start.
    #[test]
    fn every_job_key_has_a_valid_policy() {
        for key in JobKey::ALL {
            let spec = policy(*key);
            assert_eq!(spec.key, *key, "the policy must carry its own key");
            assert!(!spec.name.is_empty(), "{key:?} has no display name");
            assert!(
                spec.max_concurrent > 0,
                "{key:?} would allow no concurrent run"
            );
            spec.validate()
                .unwrap_or_else(|e| panic!("{key:?} has an invalid policy: {e}"));
        }
    }

    #[test]
    fn the_catalog_covers_exactly_the_declared_keys() {
        assert_eq!(all().len(), JobKey::ALL.len());
        let names: Vec<&str> = all().iter().map(|s| s.name).collect();
        for name in &names {
            assert!(!name.is_empty());
        }
    }

    /// Keeps a destructive job out of crash recovery, asserted against the real
    /// catalog rather than a synthetic spec.
    #[test]
    fn destructive_jobs_are_never_durable_retried_or_interrupted() {
        for key in [JobKey::Copy, JobKey::Move] {
            let spec = policy(key);
            assert!(!spec.idempotent, "{key:?} is not idempotent");
            assert_eq!(spec.durability, Durability::Memory, "{key:?}");
            assert_eq!(spec.retry, RetryPolicy::Never, "{key:?}");
            assert!(!spec.resumable, "{key:?} must not be resumable");
            assert!(!spec.cancellable, "{key:?} must not be cancellable");
            assert!(spec.chunkable.is_none(), "{key:?} cannot be interrupted");
            assert!(spec.quiet.is_none(), "{key:?} must not be deferred");
        }
    }

    /// Deferral is only declared where something can act on it.
    #[test]
    fn only_background_jobs_are_deferrable() {
        for key in JobKey::ALL {
            let spec = policy(*key);
            if spec.quiet.is_some() {
                assert_eq!(
                    spec.priority,
                    Priority::Background,
                    "{key:?} defers but is not a background job"
                );
                assert!(
                    spec.chunkable.is_some(),
                    "{key:?} declares deferral but cannot act on it"
                );
            }
        }
    }

    /// A job whose lateness has a cost must never be gated on idle.
    #[test]
    fn latency_sensitive_jobs_are_never_deferred() {
        let key = JobKey::IndexCommit;
        assert!(
            policy(key).quiet.is_none(),
            "{key:?} is latency sensitive and must run on time"
        );
    }

    /// A deferral cap of zero means "may never run": only honest for work nobody
    /// is waiting for.
    #[test]
    fn a_zero_deferral_cap_implies_the_job_is_never_forced() {
        for key in JobKey::ALL {
            let spec = policy(*key);
            if let Some(quiet) = spec.quiet
                && quiet.max_deferral_hours == 0
            {
                assert!(
                    !spec.force_safe,
                    "{key:?} claims it may be forced while also never being forced"
                );
            }
        }
    }

    /// GC must not be forced to run while uploads are in flight.
    #[test]
    fn gc_is_deferrable_but_never_forced() {
        let gc = policy(JobKey::GarbageCollection);
        assert!(gc.quiet.is_some(), "GC should wait for a quiet moment");
        assert!(!gc.force_safe, "forcing GC races a concurrent upload");
        assert!(gc.chunkable.is_some());
    }

    #[test]
    fn copy_and_move_are_owner_only_and_reindex_allows_an_admin() {
        assert_eq!(policy(JobKey::Copy).visibility, Visibility::Owner);
        assert_eq!(policy(JobKey::Move).visibility, Visibility::Owner);
        assert_eq!(policy(JobKey::Reindex).visibility, Visibility::OwnerOrAdmin);
    }

    #[test]
    fn concurrency_caps_are_declared_per_job() {
        assert_eq!(policy(JobKey::Copy).max_concurrent, 8);
        assert_eq!(policy(JobKey::Move).max_concurrent, 8);
        assert_eq!(policy(JobKey::Reindex).max_concurrent, 4);
        assert_eq!(policy(JobKey::GarbageCollection).max_concurrent, 1);
    }

    /// The interval is a reaction time, so it is pinned here: a job that fires
    /// every 30 seconds also decides how much of the run list is about it.
    #[test]
    fn the_intervals_are_the_ones_this_catalog_argues_for() {
        let interval = |key| match policy(key).trigger {
            Trigger::Periodic { interval_secs, .. } => interval_secs,
            other => panic!("{key:?} is not periodic: {other:?}"),
        };
        assert_eq!(
            interval(JobKey::IndexCommit),
            120,
            "a backstop, not a poller"
        );
        assert_eq!(interval(JobKey::PasswordCacheCleanup), 900);
        assert_eq!(interval(JobKey::TokenExpiryCheck), 3600);
        assert_eq!(interval(JobKey::ExpiredDataCleanup), 3600);
        assert_eq!(interval(JobKey::IndexBackfill), 300);
        assert_eq!(interval(JobKey::ZipTaskCleanup), 600);
    }

    /// Only a pass that usually finds nothing is allowed to keep no history for
    /// an idle tick, and nothing that must be replayed after a crash may.
    #[test]
    fn only_the_passes_that_usually_find_nothing_are_notable() {
        let notable: Vec<JobKey> = JobKey::ALL
            .iter()
            .copied()
            .filter(|key| policy(*key).history == History::Notable)
            .collect();
        assert_eq!(
            notable,
            vec![
                JobKey::TokenExpiryCheck,
                JobKey::PasswordCacheCleanup,
                JobKey::ExpiredDataCleanup,
                JobKey::IndexCommit,
                JobKey::IndexBackfill,
                JobKey::ZipTaskCleanup,
            ]
        );
        for key in notable {
            assert_eq!(
                policy(key).durability,
                Durability::Audit,
                "{key:?} keeps no queued row but is not audit-only"
            );
        }
    }

    /// Every run a request or an administrator can start is recorded from the
    /// moment it is submitted, so a client polling it always finds something.
    #[test]
    fn a_run_somebody_asked_for_is_journalled_from_the_start() {
        for key in [JobKey::Copy, JobKey::Move, JobKey::Reindex] {
            assert_eq!(
                policy(key).history,
                History::EveryRun,
                "{key:?} is submitted by a caller and must leave a row"
            );
        }
    }
}
