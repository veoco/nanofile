//! What a job is: how it is triggered, how hard it may push, how long its
//! record is kept, and what an administrator is allowed to see. Every tunable
//! lives in one [`JobSpec`] in [`super::catalog`], which the executor reads.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use super::context::JobContext;
use super::run::{JobFailure, Outcome, Params};

/// Stable identity of a job, independent of its display name: the map key, the
/// log field and the administrator URL, so renaming never orphans a stored run.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum JobKey {
    Copy,
    Move,
    Reindex,
    /// Index a batch of files a mutation produced (an upload, a sync commit, a
    /// rename). One run per *batch* is deliberate: active runs are never evicted
    /// from the run table, so one per file would grow it without bound on a sync.
    IndexFiles,
    TokenExpiryCheck,
    PasswordCacheCleanup,
    /// One pass for everything whose only record is an expiry date: tokens,
    /// share links, upload links and abandoned temporary uploads.
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

    /// Locale key of this job's display name, derived from the slug: one place
    /// decides the spelling, and the translation-coverage test walks the declared
    /// keys, so a job cannot be added without a name everywhere.
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
/// The journal outlives the job set, and a run list printing a retired slug
/// would show code vocabulary. Only the name stays in the dictionary: a retired
/// job has no description, because it is not on the registry page.
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
/// A service is not a job: it has no owner, no progress and no terminal state.
/// It still needs an identity of its own so the admin listing can name it in the
/// reader's language rather than the name it was spawned with.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum ServiceKey {
    /// Watches for the events that send mail.
    EventListener,
    /// Delivers the outbox, and retries what could not be delivered. A queue
    /// insertion wakes it; the timer only paces retries.
    MailOutbox,
}

impl ServiceKey {
    /// Every key, so the admin listing's translation coverage is a test.
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

/// Why a job the catalog declares is not registered on this server, so an
/// administrator can tell "not run here" from "this page forgot it".
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
    /// Every reason, so the admin page's translation coverage is a test.
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
/// Carries a name key rather than a job key: the thing that is missing may be a
/// service, and a panel that could only name missing jobs would stay silent.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SkippedTask {
    /// Stable slug, so the row has a handle that is not its label.
    pub slug: &'static str,
    /// Locale key of the name it is known by.
    pub name_key: String,
    pub reason: SkipReason,
}

impl SkippedTask {
    pub fn job(key: JobKey, reason: SkipReason) -> Self {
        Self {
            slug: key.as_str(),
            name_key: key.name_key(),
            reason,
        }
    }

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
    /// Runs once at startup as a background job, not blocking `AppState` construction.
    StartupOnly,
    /// Submitted by a request. The handler owns admission, not the clock.
    OnDemand,
}

/// What to do when a periodic tick arrives while the previous run is active,
/// checked before a concurrency permit is taken so a skipped tick costs nothing.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum OverlapPolicy {
    /// Leave the tick unserved: a job that cannot keep up should not pile up.
    #[default]
    Skip,
    /// Start anyway, bounded by `max_concurrent`.
    Queue,
    /// Always start a fresh run.
    Allow,
}

/// Who a job competes with. The pools are separate because a background job that
/// shares one with an interactive job can block it for its whole duration, and
/// tokio cannot preempt it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Priority {
    /// Request-scoped work whose caller is waiting: overload is a 429, not a delay.
    Interactive,
    Normal,
    /// Heavy work that may be deferred while the server is busy; a small dedicated pool.
    Background,
}

impl Priority {
    /// Whether a `QuietPolicy` may be attached; deferring interactive work only delays a failure.
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

/// How finely a job can be interrupted: tokio cannot suspend a running task, so
/// a job only steps aside at a checkpoint, and one without a `ChunkPolicy`
/// cannot honour a `QuietPolicy` — the catalog refuses that at startup.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ChunkPolicy {
    /// Upper bound on how long one indivisible unit may run — this *is* the
    /// reaction time, enforced at the checkpoint even when the server is quiet.
    /// `0` means the job did not bound its unit.
    pub max_chunk_ms: u64,
    /// Name of one unit, for logs and the admin page.
    pub unit: &'static str,
}

/// What to do when load rises around a running job.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum SpikePolicy {
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
    /// starts; dwell keeps it from starting in a gap between two requests.
    pub min_idle_for_secs: u64,
    /// After this long spent deferred, run anyway — unless the job is not
    /// `force_safe`, then report instead. `0` means never force (optional work).
    pub max_deferral_hours: u64,
    pub on_spike: SpikePolicy,
}

/// How long a run may take, and how long it may stall. A single wall-clock
/// timeout fits neither a healthy long GC nor a conversion, so the progress
/// watchdog catches what matters: a run that has stopped making progress.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TimeoutPolicy {
    /// Fail the run when no progress has been reported for this long.
    pub no_progress_for_secs: Option<u64>,
    /// Hard ceiling regardless of progress.
    pub max_total_secs: Option<u64>,
}

/// How a failed run is retried. Retrying is only safe for an idempotent job: the
/// catalog refuses a retry policy on anything whose rerun would not be harmless.
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
    /// Total attempts allowed, including the first; `0` is rejected by
    /// [`JobSpec::validate`], never treated as "once".
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
    /// Reject a second active run whose `params[name]` is equal.
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

/// How much of a job's run belongs in the history: an event somebody asked for
/// gets a row per run, a heartbeat on a timer does not, because "nothing was
/// expired" rows bury the rows that matter.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum History {
    /// A row per run: queued, running, then terminal — for anything somebody
    /// asked for, and for a pass too seldom for its rows to crowd anything out.
    #[default]
    EveryRun,
    /// Only the runs with something to report: an idle tick leaves no row but is
    /// still counted in the lifetime totals, while a failure, real work or a
    /// manual run is always recorded.
    Notable,
}

impl History {
    /// Whether a run of this kind is written down before it starts. A `Notable`
    /// job writes one terminal row instead, so an idle tick costs no database
    /// write.
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

/// A job body: pure with respect to the task system, driven by a [`JobContext`]
/// the executor owns for the life of a run; the generation's resources are
/// captured by the closure, not threaded through [`Params`].
pub type JobRunFn = Arc<
    dyn Fn(JobContext, Params) -> Pin<Box<dyn Future<Output = Result<Outcome, JobFailure>> + Send>>
        + Send
        + Sync,
>;

/// Everything the executor needs to know about one job, apart from its body.
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
    /// Whether `max_deferral` may force this job to run while the server is busy;
    /// `false` where a forced run is unsafe (GC racing a concurrent upload).
    pub force_safe: bool,
    pub quiet: Option<QuietPolicy>,
    /// Whether re-running is harmless. Gates `Durability::Durable`.
    pub idempotent: bool,
}

/// Why a spec was rejected at startup.
#[derive(Debug, PartialEq, Eq)]
pub enum SpecError {
    /// A `QuietPolicy` on a job with no checkpoint: the policy would be a lie.
    DeferrableWithoutChunks(JobKey),
    /// A `QuietPolicy` on a job that is never deferred.
    QuietOnNonBackground(JobKey),
    /// `Durable` without idempotency: a replayed destructive run is data loss.
    DurableWithoutIdempotency(JobKey),
    /// `Notable` while `Durable`: a replayable run must be written down when it
    /// starts.
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
    /// The invariants that must hold for every registered job, checked at
    /// startup so a mis-declared job fails loudly rather than silently or
    /// dangerously.
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

    /// A run replayed after a crash must have been written down when it started.
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
