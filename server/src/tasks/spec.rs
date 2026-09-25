//! What a job is: how it is triggered, how hard it may push, how long its
//! record is kept, and what an administrator is allowed to see.
//!
//! Every job's tunables live in one [`JobSpec`] declared in
//! [`super::catalog`]. That table is the only place a job's concurrency, retry
//! and retention are written down; the executor reads it and nothing else does.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use super::context::JobContext;
use super::run::{JobFailure, Outcome, Params};

/// Stable identity of a job, independent of its display name.
///
/// Used as the map key, the log field and the administrator URL, so renaming a
/// job for display never orphans a stored run.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum JobKey {
    // ── Request-submitted jobs ───────────────────────────────────────────
    Copy,
    Move,
    Reindex,
    /// Index a batch of files a file mutation just produced (an upload, a sync
    /// commit, a rename). Submitting a run per *batch* rather than per file is
    /// deliberate: active runs are never evicted from the run table, so one run
    /// per file would grow it without bound on a mass sync.
    IndexFiles,
    // ── Housekeeping ─────────────────────────────────────────────────────
    TokenExpiryCheck,
    PasswordCacheCleanup,
    /// One pass for everything whose only record is an expiry date: tokens,
    /// share links, upload links and abandoned temporary uploads. They were
    /// four jobs with four timers doing the same kind of work, and their
    /// summaries said nothing a reader could act on.
    ExpiredDataCleanup,
    GarbageCollection,
    BlockEncryptionConvert,
    IndexCommit,
    /// The low-load pass that finds files the index has never seen, or whose
    /// document was written by an older extractor, and indexes them.
    IndexBackfill,
    ZipTaskCleanup,
}

impl JobKey {
    /// Every key, for the catalog-coverage test and the admin listing.
    pub const ALL: &'static [JobKey] = &[
        Self::Copy,
        Self::Move,
        Self::Reindex,
        Self::IndexFiles,
        Self::TokenExpiryCheck,
        Self::PasswordCacheCleanup,
        Self::ExpiredDataCleanup,
        Self::GarbageCollection,
        Self::BlockEncryptionConvert,
        Self::IndexCommit,
        Self::IndexBackfill,
        Self::ZipTaskCleanup,
    ];

    /// Look a key up by its stored slug, for recovery from the run table.
    pub fn from_slug(slug: &str) -> Option<Self> {
        Self::ALL.iter().copied().find(|key| key.as_str() == slug)
    }

    /// Locale key of this job's display name.
    ///
    /// Derived from the slug rather than tabulated, as the settings catalog
    /// derives `setting.<key>`: one place decides how a name is spelled, and the
    /// translation-coverage test walks the declared keys, so a job cannot be
    /// added without a name in every language.
    pub fn name_key(self) -> String {
        format!("admin.job_{}", self.as_str().replace('-', "_"))
    }

    /// Locale key of the one line saying what this job does.
    pub fn desc_key(self) -> String {
        format!("{}_desc", self.name_key())
    }

    /// Stable slug used in URLs, logs and stored history.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Copy => "copy",
            Self::Move => "move",
            Self::Reindex => "reindex",
            Self::IndexFiles => "index-files",
            Self::TokenExpiryCheck => "token-expiry-check",
            Self::PasswordCacheCleanup => "password-cache-cleanup",
            Self::ExpiredDataCleanup => "expired-data-cleanup",
            Self::GarbageCollection => "gc",
            Self::BlockEncryptionConvert => "block-encryption-convert",
            Self::IndexCommit => "index-commit",
            Self::IndexBackfill => "index-backfill",
            Self::ZipTaskCleanup => "zip-task-cleanup",
        }
    }
}

/// Jobs the catalog no longer declares, with the locale key of the name they
/// were known by.
///
/// The journal outlives the job set: merging or removing a job leaves rows
/// carrying a slug nothing resolves any more, and a run list that printed the
/// slug would show code vocabulary to a person reading history. The names stay
/// in the dictionary for exactly these rows, and only the name — a retired job
/// has no description to show, because it is not on the registry page.
pub const RETIRED_JOBS: &[(&str, &str)] = &[
    ("expired-token-cleanup", "admin.job_expired_token_cleanup"),
    ("share-link-cleanup", "admin.job_share_link_cleanup"),
    ("upload-link-cleanup", "admin.job_upload_link_cleanup"),
    ("temp-upload-cleanup", "admin.job_temp_upload_cleanup"),
];

/// The locale key a retired slug was named by, if it was retired.
pub fn retired_job_name_key(slug: &str) -> Option<&'static str> {
    RETIRED_JOBS
        .iter()
        .find(|(retired, _)| *retired == slug)
        .map(|(_, key)| *key)
}

/// Stable identity of a long-lived service.
///
/// A service is deliberately not a job: it has no owner, no progress and no
/// terminal state, so it is registered with a lifecycle rather than a run
/// record. It still needs an identity of its own, so the admin listing can name
/// it in the reader's language instead of printing the name it was spawned
/// with.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum ServiceKey {
    /// Watches for the events that send mail.
    EventListener,
    /// Delivers the outbox, and retries what could not be delivered.
    ///
    /// A service rather than a job because that is what it is: a loop with no
    /// owner, no progress and no terminal state. As a job it was a 30-second
    /// poller that mostly found nothing to do and queued mail waited out its
    /// tick; now a queue insertion wakes it and the timer only paces retries.
    MailOutbox,
}

impl ServiceKey {
    /// Every key, so the admin listing's translation coverage is a test rather
    /// than a hope.
    pub const ALL: &'static [ServiceKey] = &[Self::EventListener, Self::MailOutbox];

    /// Stable slug, used in logs and the listing's DOM handle.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::EventListener => "event-listener",
            Self::MailOutbox => "mail-outbox",
        }
    }

    /// Locale key of this service's display name.
    pub fn name_key(self) -> String {
        format!("admin.service_{}", self.as_str().replace('-', "_"))
    }

    /// Locale key of the one line saying what this service does.
    pub fn desc_key(self) -> String {
        format!("{}_desc", self.name_key())
    }
}

/// Why a job the catalog declares is not registered on this server.
///
/// Jobs whose subsystem is switched off are simply not registered, which is the
/// right behaviour and also invisible: an administrator looking for garbage
/// collection cannot tell "this server does not run it" from "this page forgot
/// it". Recording the reason is what makes the two different.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum SkipReason {
    /// No notification manager, so nothing to expire tokens against.
    NotificationsOff,
    /// `[gc] enabled = false`.
    GcDisabled,
    /// Block encryption is not in lazy mode, so no plaintext blocks can exist.
    EncryptionNotLazy,
    /// No full-text indexer, so there is no index to commit.
    IndexOff,
    /// `index.backfill_enabled = false`.
    IndexBackfillOff,
    /// No mailer, so there is no outbox to drain.
    MailOff,
}

impl SkipReason {
    /// Every reason, so the admin page's translation coverage is a test rather
    /// than a hope.
    pub const ALL: &'static [SkipReason] = &[
        Self::NotificationsOff,
        Self::GcDisabled,
        Self::EncryptionNotLazy,
        Self::IndexOff,
        Self::IndexBackfillOff,
        Self::MailOff,
    ];
}

/// A job or service the catalog declares that this generation did not register.
///
/// Carries the locale key of its name rather than a job key, because the thing
/// that is missing may be a service: the mail outbox is not a job, and a panel
/// that could only name missing jobs would have to stay silent about it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SkippedTask {
    /// Stable slug, so the row has a handle that is not its label.
    pub slug: &'static str,
    /// Locale key of the name it is known by.
    pub name_key: String,
    pub reason: SkipReason,
}

impl SkippedTask {
    /// A job this server did not register.
    pub fn job(key: JobKey, reason: SkipReason) -> Self {
        Self {
            slug: key.as_str(),
            name_key: key.name_key(),
            reason,
        }
    }

    /// A service this server did not start.
    pub fn service(key: ServiceKey, reason: SkipReason) -> Self {
        Self {
            slug: key.as_str(),
            name_key: key.name_key(),
            reason,
        }
    }
}

/// How a job comes to run.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Trigger {
    /// Runs on a fixed interval, managed by the executor.
    Periodic {
        interval_secs: u64,
        overlap: OverlapPolicy,
    },
    /// Never runs on its own; only when an administrator triggers it.
    Manual,
    /// Runs once, at startup, as a background job rather than blocking
    /// `AppState` construction.
    StartupOnly,
    /// Submitted by a request. The handler owns admission, not the clock.
    OnDemand,
}

/// What to do when a periodic tick arrives while the previous run is active.
///
/// Checked before a concurrency permit is taken, so a skipped tick costs
/// nothing.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum OverlapPolicy {
    /// Leave the tick unserved. The default: a job that cannot keep up with its
    /// interval should not pile up behind itself.
    #[default]
    Skip,
    /// Start anyway, bounded by `max_concurrent`.
    Queue,
    /// Always start a fresh run.
    Allow,
}

/// Who a job competes with.
///
/// The pools are separate on purpose: a background job that shares a pool with
/// an interactive one can block it for its whole duration, and tokio cannot
/// preempt it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Priority {
    /// Request-scoped work whose caller is waiting. Never deferred; overload
    /// means a rate limit or a 429, not a delay.
    Interactive,
    /// Ordinary background work.
    Normal,
    /// Heavy work that may be deferred while the server is busy, and which
    /// competes for a small dedicated pool.
    Background,
}

impl Priority {
    /// Whether a `QuietPolicy` may be attached. Deferring an interactive job
    /// would just be a slower failure for a waiting caller.
    pub const fn is_deferrable(self) -> bool {
        matches!(self, Self::Background)
    }
}

/// The resource a job mostly contends for, used to pick a load signal.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Resource {
    BlockIo,
    Cpu,
    DbWrite,
}

/// How finely a job can be interrupted.
///
/// Load awareness is a contract, not a scheduler feature: tokio cannot suspend
/// a running task, so a job only steps aside because it checks a checkpoint.
/// A job without a `ChunkPolicy` therefore cannot honour a `QuietPolicy`, and
/// the catalog refuses that combination at startup.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ChunkPolicy {
    /// Upper bound on how long one indivisible unit may run. This *is* the
    /// reaction time: the executor can only notice load at a checkpoint.
    ///
    /// Enforced at the checkpoint itself, which hands the runtime back once
    /// this much time has passed since the previous one — so the bound holds
    /// even while the server is quiet, when a checkpoint has nothing else to
    /// do. `0` means the job did not bound its unit.
    pub max_chunk_ms: u64,
    /// Name of one unit, for logs and the admin page.
    pub unit: &'static str,
}

/// What to do when load rises around a running job.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum SpikePolicy {
    /// Keep going: the job is not the problem.
    #[default]
    Continue,
    /// Shrink the work unit via [`super::context::JobContext::budget`].
    Throttle,
    /// Park at the next checkpoint until the server is calm again.
    Yield,
}

/// When a job may run, and what stops it from never running.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct QuietPolicy {
    /// Load must have been below every threshold for this long before the job
    /// starts. Dwell is what stops a job from starting in a gap between two
    /// requests and then having to yield immediately.
    pub min_idle_for_secs: u64,
    /// After this long spent deferred, run anyway — unless the job is not
    /// `force_safe`, in which case report instead. `0` means "never force":
    /// correct for optional work.
    pub max_deferral_hours: u64,
    pub on_spike: SpikePolicy,
}

/// How long a run may take, and how long it may stall.
///
/// A single wall-clock timeout is the wrong shape for GC or a block
/// conversion: either it is short enough to kill healthy long runs, or long
/// enough to be useless. The progress watchdog catches what actually matters —
/// a run that has stopped making progress.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TimeoutPolicy {
    /// Fail the run when no progress has been reported for this long.
    pub no_progress_for_secs: Option<u64>,
    /// Hard ceiling regardless of progress.
    pub max_total_secs: Option<u64>,
}

/// How a failed run is retried.
///
/// Retrying is only safe for an idempotent job; the catalog refuses
/// `Durable` for a job that is not marked idempotent, and a retry policy on a
/// destructive job is a bug rather than a tuning choice.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum RetryPolicy {
    #[default]
    Never,
    Fixed {
        attempts: u32,
        delay_secs: u64,
    },
    Backoff {
        attempts: u32,
        base_secs: u64,
        max_secs: u64,
    },
}

impl RetryPolicy {
    /// Total attempts allowed, including the first.
    ///
    /// May be `0` for a mis-declared policy, which [`JobSpec::validate`]
    /// rejects rather than silently treating as "once".
    pub fn attempts(self) -> u32 {
        match self {
            Self::Never => 1,
            Self::Fixed { attempts, .. } | Self::Backoff { attempts, .. } => attempts,
        }
    }

    /// Delay before attempt `attempt` (1-based). `attempt == 1` never retries.
    pub fn delay_secs(self, attempt: u32) -> u64 {
        match self {
            Self::Never => 0,
            Self::Fixed { delay_secs, .. } => delay_secs,
            Self::Backoff {
                base_secs,
                max_secs,
                ..
            } => {
                let factor = 1u64
                    .checked_shl(attempt.saturating_sub(1).min(20))
                    .unwrap_or(u64::MAX);
                base_secs.saturating_mul(factor).min(max_secs)
            }
        }
    }
}

/// What makes two submissions of the same job the same piece of work.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Dedup {
    #[default]
    None,
    /// Reject a second active run whose `params[name]` is equal. Replaces the
    /// hand-rolled per-repo reindex lock.
    ByParams(&'static str),
}

/// Whether a run is kept only in memory, recorded, or resumable.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Default)]
pub enum Durability {
    /// In-process only. Correct for anything destructive or non-idempotent.
    #[default]
    Memory,
    /// Terminal state is written to the database so history survives a crash.
    /// No automatic retry, so idempotency is not required.
    Audit,
    /// Queued/running runs are persisted and recovered after a crash. Requires
    /// `idempotent` — the catalog refuses the combination otherwise.
    Durable,
}

/// How much of a job's run belongs in the history.
///
/// The task system records two different kinds of work: what somebody asked
/// for, and what a timer does on its own. A copy, a move and a reindex are
/// events — a run happened, and its record is the account of it. A cleanup pass
/// on a 15-minute timer is a heartbeat: it fires far more often than it has
/// work, and a row saying "nothing was expired" is noise that buries the rows
/// that matter. This is where a job says which of the two it is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum History {
    /// A row per run: queued, running, then terminal. For anything somebody
    /// asked for, and for a pass that runs too seldom for its rows to crowd
    /// anything out.
    #[default]
    EveryRun,
    /// Only the runs with something to report.
    ///
    /// A scheduled run that found nothing to do leaves no row and no run in
    /// memory — it is still counted in the job's lifetime totals, which is what
    /// says the job is ticking. A run that failed, or that did work, is always
    /// recorded, and so is a run somebody asked for by hand: the record is the
    /// answer to the press.
    Notable,
}

impl History {
    /// Whether a run of this kind is written down before it starts.
    ///
    /// Only an `EveryRun` job gets the queued/running rows; a `Notable` one
    /// writes a single terminal row when it has something to say, which is what
    /// makes an idle tick cost no database write at all.
    pub const fn journals_from_the_start(self) -> bool {
        matches!(self, Self::EveryRun)
    }
}

/// Who may read a run's record.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Visibility {
    /// Only the submitting user. No administrator override, matching the
    /// copy/move progress endpoint's long-standing 404-for-everyone-else.
    #[default]
    Owner,
    OwnerOrAdmin,
    Admin,
}

/// A job body: pure with respect to the task system, driven by a [`JobContext`].
///
/// Boxed and `'static` because the executor owns it for the life of a run; the
/// generation's resources are captured by the closure, not threaded through
/// [`Params`].
pub type JobRunFn = Arc<
    dyn Fn(JobContext, Params) -> Pin<Box<dyn Future<Output = Result<Outcome, JobFailure>> + Send>>
        + Send
        + Sync,
>;

/// Everything the executor needs to know about one job, apart from its body.
///
/// Declared in [`super::catalog`]; the body is paired with it in `tasks::setup`.
pub struct JobSpec {
    pub key: JobKey,
    /// Human-readable name for logs and the admin page.
    pub name: &'static str,
    pub trigger: Trigger,
    pub priority: Priority,
    pub resource: Resource,
    /// Simultaneous runs of *this* job.
    pub max_concurrent: usize,
    /// How many queued runs may exist per submitter before a 429. `0` means
    /// unlimited (bounded only by the store's retention).
    pub queue_depth: usize,
    pub timeout: TimeoutPolicy,
    pub retry: RetryPolicy,
    pub dedup: Dedup,
    pub visibility: Visibility,
    pub durability: Durability,
    /// How much of each run survives in the history.
    pub history: History,
    /// Whether a run may be interrupted at a checkpoint without harm.
    pub resumable: bool,
    /// Whether a caller may ask for a run to stop.
    pub cancellable: bool,
    /// `None` means the job cannot honour a `QuietPolicy`.
    pub chunkable: Option<ChunkPolicy>,
    /// Whether `max_deferral` may force this job to run while the server is
    /// busy. `false` for anything whose forced execution is unsafe (GC would
    /// race a concurrent upload).
    pub force_safe: bool,
    pub quiet: Option<QuietPolicy>,
    /// Whether re-running is harmless. Gates `Durability::Durable`.
    pub idempotent: bool,
}

/// Why a spec was rejected at startup.
#[derive(Debug, PartialEq, Eq)]
pub enum SpecError {
    /// A `QuietPolicy` on a job with no checkpoint. The executor could not make
    /// it step aside, so the policy would be a lie.
    DeferrableWithoutChunks(JobKey),
    /// A `QuietPolicy` on a job that is never deferred.
    QuietOnNonBackground(JobKey),
    /// `Durable` without idempotency: after a crash the run would be replayed,
    /// and for a destructive job that is data loss.
    DurableWithoutIdempotency(JobKey),
    /// `Notable` while `Durable`: a run that must be replayed after a crash has
    /// to have been written down before it started, or there is nothing to
    /// replay and nothing to report.
    NotableButReplayable(JobKey),
    /// A retry policy on a job whose rerun is not harmless.
    RetryOnNonIdempotent(JobKey),
    /// Zero total attempts would mean the job never runs.
    NoAttempts(JobKey),
}

impl std::fmt::Display for SpecError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::DeferrableWithoutChunks(k) => write!(
                f,
                "job {:?} declares a quiet policy but has no chunk policy: load awareness \
                 needs a checkpoint to act on",
                k
            ),
            Self::QuietOnNonBackground(k) => write!(
                f,
                "job {:?} declares a quiet policy but is not a background job; deferring \
                 interactive work only makes a waiting caller wait longer",
                k
            ),
            Self::DurableWithoutIdempotency(k) => write!(
                f,
                "job {:?} is durable but not marked idempotent: replaying it after a crash \
                 would not be safe",
                k
            ),
            Self::NotableButReplayable(k) => write!(
                f,
                "job {:?} keeps only the runs worth reporting but is durable: a run that \
                 must be replayed after a crash has to be written down when it starts",
                k
            ),
            Self::RetryOnNonIdempotent(k) => {
                write!(f, "job {:?} retries but is not marked idempotent", k)
            }
            Self::NoAttempts(k) => write!(f, "job {:?} allows no attempts", k),
        }
    }
}

impl JobSpec {
    /// The invariants that must hold for every registered job.
    ///
    /// Checked once at startup so a mis-declared job fails loudly instead of
    /// silently doing nothing (a `QuietPolicy` that never fires) or something
    /// dangerous (a durable, destructive job replayed after a crash).
    pub fn validate(&self) -> Result<(), SpecError> {
        if self.retry.attempts() == 0 {
            return Err(SpecError::NoAttempts(self.key));
        }
        if self.quiet.is_some() {
            if self.chunkable.is_none() {
                return Err(SpecError::DeferrableWithoutChunks(self.key));
            }
            if !self.priority.is_deferrable() {
                return Err(SpecError::QuietOnNonBackground(self.key));
            }
        }
        if self.durability == Durability::Durable && !self.idempotent {
            return Err(SpecError::DurableWithoutIdempotency(self.key));
        }
        if self.durability == Durability::Durable && !self.history.journals_from_the_start() {
            return Err(SpecError::NotableButReplayable(self.key));
        }
        if self.retry != RetryPolicy::Never && !self.idempotent {
            return Err(SpecError::RetryOnNonIdempotent(self.key));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(key: JobKey) -> JobSpec {
        JobSpec {
            key,
            name: "test",
            trigger: Trigger::Manual,
            priority: Priority::Normal,
            resource: Resource::Cpu,
            max_concurrent: 1,
            queue_depth: 0,
            timeout: TimeoutPolicy::default(),
            retry: RetryPolicy::Never,
            dedup: Dedup::None,
            visibility: Visibility::Owner,
            durability: Durability::Memory,
            history: History::EveryRun,
            resumable: true,
            cancellable: true,
            chunkable: None,
            force_safe: false,
            quiet: None,
            idempotent: true,
        }
    }

    #[test]
    fn a_quiet_policy_without_chunks_is_rejected() {
        let mut s = spec(JobKey::GarbageCollection);
        s.priority = Priority::Background;
        s.quiet = Some(QuietPolicy {
            min_idle_for_secs: 30,
            max_deferral_hours: 24,
            on_spike: SpikePolicy::Yield,
        });
        assert_eq!(
            s.validate(),
            Err(SpecError::DeferrableWithoutChunks(
                JobKey::GarbageCollection
            ))
        );

        s.chunkable = Some(ChunkPolicy {
            max_chunk_ms: 200,
            unit: "repo",
        });
        assert_eq!(s.validate(), Ok(()));
    }

    #[test]
    fn a_quiet_policy_on_an_interactive_job_is_rejected() {
        let mut s = spec(JobKey::Reindex);
        s.priority = Priority::Interactive;
        s.chunkable = Some(ChunkPolicy {
            max_chunk_ms: 200,
            unit: "file",
        });
        s.quiet = Some(QuietPolicy {
            min_idle_for_secs: 30,
            max_deferral_hours: 24,
            on_spike: SpikePolicy::Yield,
        });
        assert_eq!(
            s.validate(),
            Err(SpecError::QuietOnNonBackground(JobKey::Reindex))
        );
    }

    /// The rule that keeps a destructive job out of crash recovery.
    #[test]
    fn durable_requires_idempotency() {
        let mut s = spec(JobKey::Move);
        s.durability = Durability::Durable;
        s.idempotent = false;
        assert_eq!(
            s.validate(),
            Err(SpecError::DurableWithoutIdempotency(JobKey::Move))
        );

        // Audit is fine: it records, it does not replay.
        s.durability = Durability::Audit;
        assert_eq!(s.validate(), Ok(()));
    }

    #[test]
    fn retry_requires_idempotency() {
        let mut s = spec(JobKey::Copy);
        s.idempotent = false;
        s.retry = RetryPolicy::Fixed {
            attempts: 3,
            delay_secs: 1,
        };
        assert_eq!(
            s.validate(),
            Err(SpecError::RetryOnNonIdempotent(JobKey::Copy))
        );
    }

    /// A run that has to be replayed after a crash must have been written down
    /// when it started, so `Notable` and `Durable` cannot be declared together.
    #[test]
    fn a_notable_job_is_never_replayable() {
        let mut s = spec(JobKey::IndexCommit);
        s.history = History::Notable;
        s.durability = Durability::Audit;
        assert_eq!(s.validate(), Ok(()), "notable audit-only is the point");

        s.durability = Durability::Durable;
        assert_eq!(
            s.validate(),
            Err(SpecError::NotableButReplayable(JobKey::IndexCommit))
        );

        s.history = History::EveryRun;
        assert_eq!(s.validate(), Ok(()));
    }

    #[test]
    fn backoff_is_exponential_and_capped() {
        let r = RetryPolicy::Backoff {
            attempts: 10,
            base_secs: 30,
            max_secs: 3600,
        };
        assert_eq!(r.attempts(), 10);
        assert_eq!(r.delay_secs(1), 30);
        assert_eq!(r.delay_secs(2), 60);
        assert_eq!(r.delay_secs(3), 120);
        assert_eq!(r.delay_secs(9), 3600, "capped");
        assert_eq!(r.delay_secs(64), 3600, "a huge attempt cannot overflow");
    }

    #[test]
    fn never_retries_and_reports_one_attempt() {
        let r = RetryPolicy::Never;
        assert_eq!(r.attempts(), 1);
        assert_eq!(r.delay_secs(1), 0);
    }

    #[test]
    fn a_zero_attempt_policy_is_rejected() {
        let mut s = spec(JobKey::GarbageCollection);
        s.retry = RetryPolicy::Fixed {
            attempts: 0,
            delay_secs: 1,
        };
        assert_eq!(
            s.validate(),
            Err(SpecError::NoAttempts(JobKey::GarbageCollection))
        );
    }

    #[test]
    fn every_key_has_a_unique_slug() {
        let mut slugs: Vec<&str> = JobKey::ALL.iter().map(|k| k.as_str()).collect();
        let total = slugs.len();
        slugs.sort_unstable();
        slugs.dedup();
        assert_eq!(slugs.len(), total, "duplicate job slug");
    }
}
