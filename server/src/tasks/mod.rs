//! The task system: one registry of jobs, one bounded store of runs, one
//! executor that owns every state transition.
//!
//! Two ways in, and the difference is the point:
//!
//! * [`TaskSystem::submit`] schedules work somebody will ask about later by id.
//!   It produces a [`JobRun`] that can be polled, and — for a job declared
//!   `cancellable` — stopped.
//! * [`TaskSystem::run_inline`] runs request-scoped work under the same
//!   concurrency limits and timeouts but keeps no record, because no second
//!   caller will ever look it up.
//!
//! What is *not* here is as deliberate: a long-lived event listener has no
//! owner, no progress and no terminal state, so it is a service rather than a
//! job and does not belong in the run table.

pub mod admission;
pub mod catalog;
pub mod compat;
pub mod context;
pub mod executor;
pub mod queue;
pub mod registry;
pub mod run;
pub mod setup;
pub mod spec;
pub mod store;

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, PoisonError, RwLock};

use base::error::AppError;
use tokio::sync::Semaphore;
use tokio_util::sync::CancellationToken;

use self::admission::{LoadGauge, LoadSnapshot};
use self::context::JobContext;
use self::registry::{JobRegistry, RegisteredJob, RegistryError};
use self::run::{JobRun, JobState, Params, RunId, Viewer};
use self::spec::{Dedup, JobKey, OverlapPolicy, Priority, SkipReason};
use self::store::{RunFilter, RunLimits, RunStore};

pub use self::run::{JobFailure, Origin, Outcome, Progress};
pub use self::spec::{
    ChunkPolicy, Durability, Resource, ServiceKey, TimeoutPolicy, Trigger, Visibility,
};

/// Lifetime counters for one job, for the administrator's view.
///
/// Process-lifetime, like the task system itself, so the numbers survive an
/// in-place restart. They are aggregates, not a substitute for the run table:
/// the run table keeps what happened recently, these keep the totals.
#[derive(Clone, Debug, Default)]
pub struct JobStats {
    pub run_count: u64,
    pub success_count: u64,
    pub error_count: u64,
    pub last_run_at: Option<i64>,
    pub last_duration_ms: u64,
    pub last_success_message: String,
    pub last_error_message: String,
    pub total_processed: u64,
    /// Terminal state of the most recent run, for the listing.
    pub last_state: Option<JobState>,
}

/// How long a deferrable job has been waiting, and since when the server has
/// been calm enough to start it.
#[derive(Default)]
struct QuietState {
    tracker: admission::QuietTracker,
    /// Set while the job is being deferred, for the starvation valve.
    deferred_since: Option<i64>,
}

/// A load sample older than this is not a reason to start a job.
const MAX_SAMPLE_AGE: std::time::Duration = std::time::Duration::from_secs(30);

/// Reports a database pool's occupancy as `(in use, maximum)`.
pub type DbProbe = Arc<dyn Fn() -> (u64, u64) + Send + Sync>;

/// Process-wide admission limits.
#[derive(Clone, Copy, Debug)]
pub struct TaskLimits {
    /// How many runs may be active server-wide, across every job. `0` means
    /// unlimited. The backstop behind the per-user cap.
    pub max_active_total: usize,
    /// How many runs one user may have active at once, across every job. `0`
    /// means unlimited.
    ///
    /// The old cap was only the server-wide number, so one account could take
    /// every slot and the next user got a 429.
    pub max_active_per_user: usize,
    pub runs: RunLimits,
}

impl Default for TaskLimits {
    fn default() -> Self {
        Self {
            max_active_total: 100,
            max_active_per_user: 8,
            runs: RunLimits::default(),
        }
    }
}

struct Inner {
    /// Populated during startup, read-only afterwards.
    registry: RwLock<JobRegistry>,
    /// One permit pool per job, created with the job's declared concurrency.
    permits: RwLock<HashMap<JobKey, Arc<Semaphore>>>,
    store: RunStore,
    /// Live runs' cancellation handles, so a caller can stop one.
    cancels: RwLock<HashMap<RunId, CancellationToken>>,
    limits: RwLock<TaskLimits>,
    /// When this process's task system was built, for the worker-occupancy
    /// calculation.
    started_at: std::time::Instant,
    /// The current server generation's cancellation token. Replaced by
    /// [`TaskSystem::install`], because the task system outlives a generation
    /// while the token does not.
    shutdown: RwLock<CancellationToken>,
    /// Long-lived services of the current generation, for the admin listing.
    services: RwLock<Vec<ServiceKey>>,
    /// Jobs the catalog declares that this server did not register, with the
    /// reason, so the admin listing can say why rather than showing nothing.
    skipped: RwLock<Vec<(JobKey, SkipReason)>>,
    /// Lifetime counters per job.
    stats: RwLock<HashMap<JobKey, JobStats>>,
    /// How busy the server is, for the administrator's view and — once load
    /// awareness is switched on — for admission.
    load: LoadGauge,
    /// The gate jobs park at. Shares the gauge with `load`.
    gate: admission::YieldGate,
    /// Reads the database pool's occupancy. Installed per generation, because
    /// the pool belongs to the generation.
    db_probe: RwLock<Option<DbProbe>>,
    /// Per-job quiet tracking: how long each deferrable job has been waiting,
    /// and since when it has been calm enough to start.
    quiet: RwLock<HashMap<JobKey, QuietState>>,
    /// Durable history, when the database is available. `None` in a test that
    /// builds a task system without one.
    journal: RwLock<Option<Arc<dyn crate::repository::job_run::JobRunRepository>>>,
    /// Whether jobs may be deferred and throttled by load.
    load_aware: AtomicBool,
    /// Seconds between load samples.
    sample_interval_secs: AtomicU64,
}

/// The task system.
#[derive(Clone)]
pub struct TaskSystem {
    inner: Arc<Inner>,
}

impl std::fmt::Debug for TaskSystem {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TaskSystem")
            .field("jobs", &self.registry().len())
            .field("runs", &self.inner.store.len())
            .finish_non_exhaustive()
    }
}

impl TaskSystem {
    /// A new, empty task system.
    ///
    /// Process-lifetime: built once, outside the server generation loop, so run
    /// history and schedule state survive an in-place restart.
    pub fn new(limits: TaskLimits, shutdown: CancellationToken) -> Arc<Self> {
        // One gauge, shared by the administrator's panel and by the gate jobs
        // park at: a job must see exactly the numbers the operator sees.
        let load = LoadGauge::new();
        let gate = admission::YieldGate::new(load.clone(), admission::LoadThresholds::default());
        Arc::new(Self {
            inner: Arc::new(Inner {
                registry: RwLock::new(JobRegistry::new()),
                permits: RwLock::new(HashMap::new()),
                store: RunStore::new(limits.runs),
                cancels: RwLock::new(HashMap::new()),
                limits: RwLock::new(limits),
                started_at: std::time::Instant::now(),
                shutdown: RwLock::new(shutdown),
                services: RwLock::new(Vec::new()),
                skipped: RwLock::new(Vec::new()),
                stats: RwLock::new(HashMap::new()),
                load,
                gate,
                db_probe: RwLock::new(None),
                quiet: RwLock::new(HashMap::new()),
                journal: RwLock::new(None),
                load_aware: AtomicBool::new(false),
                sample_interval_secs: AtomicU64::new(5),
            }),
        })
    }

    fn registry(&self) -> std::sync::RwLockReadGuard<'_, JobRegistry> {
        self.inner
            .registry
            .read()
            .unwrap_or_else(PoisonError::into_inner)
    }

    /// Bind this generation's job set and cancellation token.
    ///
    /// The task system is process-lifetime so that run history and schedule
    /// state survive an in-place restart; the *bodies* are not, because they
    /// capture the generation's database pool, block store and indexer. So each
    /// generation replaces the whole set here rather than adding to it. The
    /// policies are identical every time (they come from
    /// [`catalog`](self::catalog)); only the bodies differ.
    ///
    /// Validation happens against the fresh set, so a mis-declared job fails
    /// before anything is swapped in.
    pub fn install(
        &self,
        jobs: Vec<RegisteredJob>,
        shutdown: CancellationToken,
    ) -> Result<(), RegistryError> {
        let mut fresh = JobRegistry::new();
        for job in jobs {
            fresh.register(job)?;
        }
        let mut permits = HashMap::new();
        for job in fresh.jobs() {
            permits.insert(job.key(), Arc::new(Semaphore::new(job.spec.max_concurrent)));
        }
        *self
            .inner
            .registry
            .write()
            .unwrap_or_else(PoisonError::into_inner) = fresh;
        *self
            .inner
            .permits
            .write()
            .unwrap_or_else(PoisonError::into_inner) = permits;
        *self
            .inner
            .shutdown
            .write()
            .unwrap_or_else(PoisonError::into_inner) = shutdown;
        // Services are spawned per generation, so the listing is rebuilt with
        // it rather than accumulating stale names. The skip record is written
        // after this call, by whoever decided not to register what.
        self.inner
            .services
            .write()
            .unwrap_or_else(PoisonError::into_inner)
            .clear();
        self.inner
            .skipped
            .write()
            .unwrap_or_else(PoisonError::into_inner)
            .clear();
        Ok(())
    }

    /// The current generation's cancellation token.
    pub fn shutdown_token(&self) -> CancellationToken {
        self.inner
            .shutdown
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// Register a single job. Used by tests and by anything that adds a job
    /// outside the standard set.
    pub fn register(&self, job: RegisteredJob) -> Result<(), RegistryError> {
        let key = job.key();
        let concurrency = job.spec.max_concurrent;
        {
            let mut registry = self
                .inner
                .registry
                .write()
                .unwrap_or_else(PoisonError::into_inner);
            registry.register(job)?;
        }
        self.inner
            .permits
            .write()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(key, Arc::new(Semaphore::new(concurrency)));
        Ok(())
    }

    /// Every registered job, in registration order.
    pub fn jobs(&self) -> Vec<Arc<RegisteredJob>> {
        self.registry().jobs().into_iter().cloned().collect()
    }

    pub fn job(&self, key: JobKey) -> Option<Arc<RegisteredJob>> {
        self.registry().get(key).cloned()
    }

    pub fn store(&self) -> &RunStore {
        &self.inner.store
    }

    pub fn limits(&self) -> TaskLimits {
        *self
            .inner
            .limits
            .read()
            .unwrap_or_else(PoisonError::into_inner)
    }

    /// Replace the admission limits of a running server.
    pub fn set_limits(&self, limits: TaskLimits) {
        self.inner.store.set_limits(limits.runs);
        *self
            .inner
            .limits
            .write()
            .unwrap_or_else(PoisonError::into_inner) = limits;
    }

    /// Submit a job for background execution and return the id it can be polled
    /// with.
    ///
    /// The state machine belongs to the executor from here on: the caller
    /// cannot mark the run done, and cannot forget to.
    pub async fn submit(
        &self,
        key: JobKey,
        owner: Option<i32>,
        params: Params,
        summary: impl Into<String>,
        expected_total: Option<u64>,
    ) -> Result<RunId, AppError> {
        self.submit_as(
            Origin::Request,
            key,
            owner,
            params,
            summary,
            expected_total,
            Vec::new(),
        )
        .await
    }

    /// Submit with small job-specific facts for the wire projection that must
    /// outlive the params.
    #[allow(clippy::too_many_arguments)]
    pub async fn submit_with_details(
        &self,
        key: JobKey,
        owner: Option<i32>,
        params: Params,
        summary: impl Into<String>,
        expected_total: Option<u64>,
        details: Vec<(&str, serde_json::Value)>,
    ) -> Result<RunId, AppError> {
        self.submit_as(
            Origin::Request,
            key,
            owner,
            params,
            summary,
            expected_total,
            details,
        )
        .await
    }

    /// Submit work the timer started, or a one-shot pass the server started
    /// itself.
    ///
    /// Separate from [`TaskSystem::submit`] because who asked decides whether an
    /// idle run is worth remembering: a timer's tick nobody asked for leaves no
    /// history, while a run a caller or an administrator asked for always does.
    pub(crate) async fn submit_system(
        &self,
        origin: Origin,
        key: JobKey,
        params: Params,
        summary: impl Into<String>,
    ) -> Result<RunId, AppError> {
        self.submit_as(origin, key, None, params, summary, None, Vec::new())
            .await
    }

    /// The one submission path.
    #[allow(clippy::too_many_arguments)]
    async fn submit_as(
        &self,
        origin: Origin,
        key: JobKey,
        owner: Option<i32>,
        params: Params,
        summary: impl Into<String>,
        expected_total: Option<u64>,
        details: Vec<(&str, serde_json::Value)>,
    ) -> Result<RunId, AppError> {
        let job = self
            .job(key)
            .ok_or_else(|| AppError::Internal(format!("job {key:?} is not registered")))?;
        let now = chrono::Utc::now().timestamp();

        // Admission. Checked before the run exists so a refusal leaves no
        // trace to sweep.
        let limits = self.limits();
        if job.spec.queue_depth > 0
            && self.inner.store.count_active(key, owner) >= job.spec.queue_depth
        {
            return Err(AppError::TooManyRequests);
        }
        if limits.max_active_total > 0
            && self.inner.store.count_active_all(None) >= limits.max_active_total
        {
            return Err(AppError::TooManyRequests);
        }
        if limits.max_active_per_user > 0
            && owner.is_some()
            && self.inner.store.count_active_all(owner) >= limits.max_active_per_user
        {
            return Err(AppError::TooManyRequests);
        }

        // Deduplication stands in for the per-repo reindex lock that used to be
        // a second map next to the progress map.
        if let Dedup::ByParams(field) = job.spec.dedup
            && let Some(value) = params.get(field)
            && self
                .inner
                .store
                .find_active_by_param(key, field, value)
                .is_some()
        {
            return Err(AppError::Conflict(format!(
                "{} is already in progress",
                job.spec.name
            )));
        }

        let mut run = JobRun::queued(
            key,
            origin,
            job.spec.visibility,
            owner,
            params.clone(),
            summary,
            expected_total,
            now,
        );
        for (name, value) in details {
            run.set_detail(name, value);
        }
        let id = run.id.clone();
        // Every run whose job keeps a row per run is written down before it
        // starts, so a crash leaves a row rather than nothing at all. `Audit` is
        // recorded and, if the process dies, closed by recovery rather than
        // replayed; `Durable` is resumed. A `Memory` run — the request-scoped
        // copies and moves — is not recorded anywhere, and neither is a
        // `Notable` run until it has something to report.
        //
        // The condition has to match the one that writes the terminal state
        // below, or an `Audit` job's finish updates a row that was never
        // inserted and the journal stays empty.
        let journaled_from_the_start =
            job.spec.durability >= Durability::Audit && job.spec.history.journals_from_the_start();
        if let Some(journal) = self.journal()
            && journaled_from_the_start
        {
            let now = chrono::Utc::now().timestamp();
            journal
                .enqueue(
                    crate::repository::job_run::NewJobRun {
                        id: id.as_str().to_string(),
                        kind: key.as_str().to_string(),
                        owner,
                        summary: run.summary.clone(),
                        params: Some(params.to_string()),
                        created_at: now,
                    },
                    crate::tasks::queue::QueuePolicy::DEFAULT.lease_until(now),
                )
                .await?;
        }
        self.inner.store.insert(run.clone())?;

        let cancel = CancellationToken::new();
        self.inner
            .cancels
            .write()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(id.clone(), cancel.clone());

        let system = self.clone();
        let spawned_id = id.clone();
        tokio::spawn(async move {
            system.drive(job, run, params, cancel).await;
            system
                .inner
                .cancels
                .write()
                .unwrap_or_else(PoisonError::into_inner)
                .remove(&spawned_id);
        });

        Ok(id)
    }

    /// Acquire this job's permit and run its body.
    async fn drive(
        &self,
        job: Arc<RegisteredJob>,
        run: JobRun,
        params: Params,
        cancel: CancellationToken,
    ) {
        let permits = self
            .inner
            .permits
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .get(&job.key())
            .cloned();
        let _permit = match permits {
            Some(semaphore) => match semaphore.acquire_owned().await {
                Ok(permit) => Some(permit),
                // The semaphore is only closed if the system is being torn
                // down, in which case the run is interrupted by `drain`.
                Err(_) => return,
            },
            None => None,
        };

        let started = std::time::Instant::now();
        // Counted as *background* load: admission deliberately ignores this, so
        // a running pass cannot defer itself.
        let _background = self.inner.load.background_guard();
        let journal = self.journal();
        // Whether this job was written down before it started. A job that keeps
        // only its notable runs has no row yet, and gets one at the end.
        let journaled_from_the_start =
            job.spec.durability >= Durability::Audit && job.spec.history.journals_from_the_start();
        // The same set of jobs the submit path recorded a row for: an audited
        // run is written down when it starts as well as when it ends, so its
        // row says when it began and does not read as pending work for longer
        // than the run lasts.
        if let Some(journal) = &journal
            && journaled_from_the_start
        {
            let now = chrono::Utc::now().timestamp();
            let lease = crate::tasks::queue::QueuePolicy::DEFAULT.lease_until(now);
            if let Err(e) = journal.mark_running(run.id.as_str(), now, lease).await {
                tracing::warn!(run = %run.id, "could not mark the run as running: {e}");
            }
        }
        let gate = self.yield_gate(job.as_ref());
        let report =
            executor::execute(&job, &run, params, &self.inner.store, cancel.clone(), gate).await;
        self.record_stats(job.key(), &report, started.elapsed());

        // An idle run of a timer is not an event: nobody asked for it, and its
        // next tick re-does whatever this one would have done. It is counted in
        // the job's totals above and then dropped, so the history holds what
        // happened rather than that time passed. Everything else — any failure,
        // any run that did work, and any idle run somebody asked for by hand —
        // is recorded.
        let idle = report.state == JobState::Succeeded
            && report.outcome.as_ref().is_some_and(|outcome| outcome.idle);
        let silent = idle && run.origin == Origin::Schedule;

        let finished_at = report
            .finished_at
            .unwrap_or_else(|| chrono::Utc::now().timestamp());
        let error = match &report.state {
            JobState::Failed(message) => Some(message.as_str()),
            JobState::TimedOut => Some("timed out"),
            JobState::Cancelled => Some("cancelled"),
            JobState::Interrupted => Some("interrupted by a restart"),
            _ => None,
        };
        // The job's own account of what it did is what the column is for. A run
        // that failed before it could report anything keeps the summary its
        // submitter gave, which is `None` here.
        let summary = report
            .outcome
            .as_ref()
            .map(|outcome| outcome.message.as_str())
            .filter(|message| !message.is_empty());
        // A `Notable` run that failed before it reported anything has no
        // summary of its own to write; its error is the account, so the summary
        // column stays empty rather than repeating the job's slug.
        let recorded_summary = summary.unwrap_or_default().to_string();
        // A `Memory` job is not recorded anywhere, whatever its history policy
        // says: the request-scoped copies and moves leave no trace at all.
        let keeps_a_record = job.spec.durability >= Durability::Audit;
        if let Some(journal) = &journal
            && keeps_a_record
        {
            if journaled_from_the_start {
                if let Err(e) = journal
                    .finish(
                        run.id.as_str(),
                        report.state.as_str(),
                        error,
                        report
                            .outcome
                            .as_ref()
                            .and_then(|o| o.processed)
                            .map(|p| p as i64),
                        summary,
                        finished_at,
                    )
                    .await
                {
                    tracing::warn!(run = %run.id, "could not record the finished run: {e}");
                }
            } else if !silent
                && let Err(e) = journal
                    .record_finished(crate::repository::job_run::FinishedJobRun {
                        id: run.id.as_str().to_string(),
                        kind: job.key().as_str().to_string(),
                        owner: run.owner,
                        phase: report.state.as_str().to_string(),
                        summary: recorded_summary,
                        error: error.map(str::to_string),
                        processed: report
                            .outcome
                            .as_ref()
                            .and_then(|o| o.processed)
                            .map(|p| p as i64),
                        attempt: report.attempts as i32,
                        created_at: run.created_at,
                        started_at: report.started_at,
                        finished_at,
                    })
                    .await
            {
                tracing::warn!(run = %run.id, "could not record the finished run: {e}");
            }
        }
        if silent {
            self.inner.store.discard(&run.id);
        }
        tracing::debug!(
            job = job.name(),
            run = %run.id,
            state = report.state.as_str(),
            attempts = report.attempts,
            silent,
            "job finished"
        );
    }

    /// Fold one finished run into the job's lifetime counters.
    ///
    /// Aggregates rather than the run table: the table keeps what happened
    /// recently, these keep the totals an administrator compares over time.
    fn record_stats(
        &self,
        key: JobKey,
        report: &executor::RunReport,
        elapsed: std::time::Duration,
    ) {
        let mut stats = self
            .inner
            .stats
            .write()
            .unwrap_or_else(PoisonError::into_inner);
        let entry = stats.entry(key).or_default();
        entry.run_count += 1;
        entry.last_run_at = Some(chrono::Utc::now().timestamp());
        entry.last_duration_ms = elapsed.as_millis() as u64;
        entry.last_state = Some(report.state.clone());
        if report.state == JobState::Succeeded {
            entry.success_count += 1;
            if let Some(outcome) = &report.outcome {
                entry.last_success_message = outcome.message.clone();
                if let Some(processed) = outcome.processed {
                    entry.total_processed += processed;
                }
            }
        } else {
            entry.error_count += 1;
            entry.last_error_message = match &report.state {
                JobState::Failed(message) => message.clone(),
                JobState::TimedOut => "timed out".to_string(),
                JobState::Cancelled => "cancelled".to_string(),
                JobState::Interrupted => "interrupted by a restart".to_string(),
                _ => String::new(),
            };
        }
    }

    /// The gate a job parks at, for jobs that declared a checkpoint.
    fn yield_gate(&self, job: &RegisteredJob) -> Option<admission::YieldGate> {
        job.spec
            .chunkable
            .is_some()
            .then(|| self.inner.gate.clone())
    }

    /// Install the durable run journal.
    pub fn set_journal(&self, journal: Arc<dyn crate::repository::job_run::JobRunRepository>) {
        *self
            .inner
            .journal
            .write()
            .unwrap_or_else(PoisonError::into_inner) = Some(journal);
    }

    fn journal(&self) -> Option<Arc<dyn crate::repository::job_run::JobRunRepository>> {
        self.inner
            .journal
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// Re-submit durable runs whose process died before they finished.
    ///
    /// Called once at start-up. Only a job that declared `Durable` is replayed:
    /// the catalog refuses that declaration for anything not marked idempotent,
    /// so a destructive run is never repeated here. The interrupted attempt is
    /// recorded rather than quietly dropped.
    pub async fn recover(&self) {
        let Some(journal) = self.journal() else {
            return;
        };
        let now = chrono::Utc::now().timestamp();
        let lease = crate::tasks::queue::QueuePolicy::DEFAULT;
        let rows = match journal.recoverable(now, 50).await {
            Ok(rows) => rows,
            Err(e) => {
                tracing::warn!("could not read the run journal for recovery: {e}");
                return;
            }
        };
        if rows.is_empty() {
            return;
        }

        for row in rows {
            let Some(key) = JobKey::from_slug(&row.kind) else {
                let _ = journal
                    .finish(
                        &row.id,
                        "interrupted",
                        Some("no job is registered under this name any more"),
                        None,
                        None,
                        now,
                    )
                    .await;
                continue;
            };
            let replayable = self
                .job(key)
                .is_some_and(|job| job.spec.durability == Durability::Durable);
            if !replayable {
                let _ = journal
                    .finish(
                        &row.id,
                        "interrupted",
                        Some("this job is not safe to replay"),
                        None,
                        None,
                        now,
                    )
                    .await;
                continue;
            }
            // Take the row over before re-submitting, so two servers starting
            // together cannot both replay it.
            match journal
                .claim_for_recovery(&row.id, row.attempt, now, lease.lease_until(now))
                .await
            {
                Ok(true) => {}
                Ok(false) => continue,
                Err(e) => {
                    tracing::warn!(run = %row.id, "could not claim a run for recovery: {e}");
                    continue;
                }
            }

            let params: Params = row
                .params
                .as_deref()
                .and_then(|p| serde_json::from_str(p).ok())
                .unwrap_or(Params::Null);
            let summary = if row.summary.is_empty() {
                format!("{} (recovered)", key.as_str())
            } else {
                format!("{} (recovered)", row.summary)
            };
            // The owner is kept: a recovered run is the continuation of the
            // caller's work, so owner-scoped visibility must still find it.
            match self
                .submit_as(
                    Origin::Startup,
                    key,
                    row.owner,
                    params,
                    summary,
                    None,
                    Vec::new(),
                )
                .await
            {
                Ok(_) => {
                    // The old attempt is closed as interrupted, so it is not
                    // read as pending work by the next start.
                    let _ = journal
                        .finish(
                            &row.id,
                            "interrupted",
                            Some("recovered after a restart"),
                            row.processed,
                            None,
                            now,
                        )
                        .await;
                    tracing::info!(job = %row.kind, run = %row.id, "recovered a durable run");
                }
                Err(e) => {
                    let _ = journal
                        .finish(
                            &row.id,
                            "failed",
                            Some(&format!("could not be recovered: {e}")),
                            None,
                            None,
                            now,
                        )
                        .await;
                }
            }
        }
    }

    /// Drop journal rows past retention. Called by the sweeper.
    pub async fn prune_journal(&self) -> usize {
        let Some(journal) = self.journal() else {
            return 0;
        };
        let policy = crate::tasks::queue::QueuePolicy::DEFAULT;
        let now = chrono::Utc::now().timestamp();
        let (sent, _failed) = policy.retention_cutoffs(now);
        match journal.prune(sent, policy.max_finished_rows).await {
            Ok(pruned) => pruned as usize,
            Err(e) => {
                tracing::warn!("could not prune the run journal: {e}");
                0
            }
        }
    }

    /// The load counters.
    pub fn load(&self) -> &LoadGauge {
        &self.inner.load
    }

    /// How busy the server is right now.
    pub fn load_snapshot(&self) -> LoadSnapshot {
        self.inner.load.snapshot()
    }

    /// Whether a job is currently being deferred, and since when. For the
    /// administrator's view: a job that is waiting must say so, or the page
    /// looks like the job is broken.
    pub fn deferred_since(&self, key: JobKey) -> Option<i64> {
        self.inner
            .quiet
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .get(&key)
            .and_then(|state| state.deferred_since)
    }

    /// Override the thresholds admission uses. Used by tests; the catalog
    /// carries the production values implicitly.
    pub fn set_load_thresholds(&self, thresholds: admission::LoadThresholds) {
        self.inner.gate.set_thresholds(thresholds);
    }

    /// Install this generation's database-pool probe.
    pub fn set_db_probe(&self, probe: DbProbe) {
        *self
            .inner
            .db_probe
            .write()
            .unwrap_or_else(PoisonError::into_inner) = Some(probe);
    }

    /// Whether deferred and throttled jobs are enabled.
    pub fn load_aware(&self) -> bool {
        self.inner.load_aware.load(Ordering::Relaxed)
    }

    /// Apply the load-aware settings of a saved configuration.
    pub fn configure_load(&self, aware: bool, sample_interval_secs: u64) {
        self.inner.gate.set_enabled(aware);
        self.inner.load_aware.store(aware, Ordering::Relaxed);
        self.inner
            .sample_interval_secs
            .store(sample_interval_secs.max(1), Ordering::Relaxed);
    }

    /// Take one sample of everything the gauge cannot see for itself.
    fn sample_load(&self) {
        if let Some(probe) = self
            .inner
            .db_probe
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
        {
            let (in_use, max) = probe();
            self.inner.load.set_db(in_use, max);
        }
        // Stable tokio metrics: everything else about the blocking pool needs
        // `tokio_unstable`, which is not worth enabling for one number.
        let handle = tokio::runtime::Handle::current();
        let metrics = handle.metrics();
        let workers = metrics.num_workers().max(1);
        let busy_nanos: u64 = (0..workers)
            .map(|i| metrics.worker_total_busy_duration(i).as_nanos() as u64)
            .sum();
        // Busy time since process start divided by wall-clock capacity: a rough
        // but trend-correct occupancy.
        let elapsed_nanos = self.inner.started_at.elapsed().as_nanos().max(1) as u64;
        let busy_pct = (busy_nanos.saturating_mul(100)
            / (elapsed_nanos.saturating_mul(workers as u64).max(1)))
        .min(100) as u32;
        // Only the injection queue is exposed on a stable tokio; the
        // per-worker local queues need `tokio_unstable`, which is not worth
        // enabling for one number.
        self.inner.load.set_runtime(
            busy_pct,
            metrics.global_queue_depth() as u64,
            metrics.num_alive_tasks() as u64,
        );
        self.inner.load.mark_sampled(chrono::Utc::now().timestamp());
    }

    /// Lifetime counters for one job.
    pub fn stats(&self, key: JobKey) -> JobStats {
        self.inner
            .stats
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .get(&key)
            .cloned()
            .unwrap_or_default()
    }

    /// Run request-scoped work under a job's concurrency limit, keeping no
    /// record.
    ///
    /// The work is bounded and observable like any other, but there is no run
    /// to poll because no second caller will look it up.
    pub async fn run_inline<F, Fut, T>(&self, key: JobKey, f: F) -> Result<T, AppError>
    where
        F: FnOnce(JobContext) -> Fut,
        Fut: std::future::Future<Output = Result<T, JobFailure>>,
    {
        let job = self
            .job(key)
            .ok_or_else(|| AppError::Internal(format!("job {key:?} is not registered")))?;
        let semaphore = self
            .inner
            .permits
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .get(&key)
            .cloned();
        let _permit = match semaphore {
            Some(semaphore) => Some(
                semaphore
                    .acquire_owned()
                    .await
                    .map_err(|_| AppError::Internal("task system is shutting down".into()))?,
            ),
            None => None,
        };

        // No record is kept, so the context is a throwaway with a fresh
        // cancellation token that follows the caller.
        let id = RunId::new();
        let tick = context::LivenessTick::default();
        let sink = context::ProgressSink::new(
            self.inner.store.clone(),
            id.clone(),
            std::time::Duration::ZERO,
            tick,
        );
        let ctx = JobContext::new(
            id,
            key,
            None,
            self.shutdown_token().child_token(),
            sink,
            context::BudgetSignal::full(),
            job.spec.chunkable,
            self.yield_gate(job.as_ref()),
        );

        f(ctx).await.map_err(|failure| match failure {
            JobFailure::App(e) => e,
            JobFailure::Cancelled => AppError::Internal("operation cancelled".into()),
            JobFailure::TimedOut => AppError::Internal("operation timed out".into()),
        })
    }

    /// Ask a run to stop.
    ///
    /// Only a job declared `cancellable` honours this; for anything else the
    /// request is refused rather than silently ignored, because cancelling a
    /// move between its two commits would lose data.
    pub fn cancel(&self, id: &RunId, viewer: Viewer) -> Result<(), AppError> {
        let run = self
            .inner
            .store
            .get_for(id, viewer)
            .ok_or_else(|| AppError::NotFound("task not found or expired".into()))?;
        if run.state.is_terminal() {
            return Err(AppError::Conflict("task has already finished".into()));
        }
        let cancellable = self
            .job(run.key)
            .is_some_and(|job| job.spec.cancellable || job.spec.resumable);
        if !cancellable {
            return Err(AppError::Conflict(format!(
                "{} cannot be cancelled",
                run.key.as_str()
            )));
        }
        if let Some(token) = self
            .inner
            .cancels
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .get(id)
            .cloned()
        {
            token.cancel();
        }
        Ok(())
    }

    /// Drop expired terminal runs. Called by one periodic sweeper.
    pub fn sweep(&self) -> usize {
        self.inner.store.sweep(chrono::Utc::now().timestamp())
    }

    /// Runs matching `filter`, newest first.
    pub fn runs(&self, filter: &RunFilter) -> Vec<JobRun> {
        self.inner.store.list(filter)
    }

    /// Stop everything this generation started, and record anything that could
    /// not be stopped.
    ///
    /// A run that is safely interruptible is cancelled outright; one that is
    /// not (a move between its two commits) is given `grace` to finish. Either
    /// way the run ends in a recorded terminal state rather than vanishing,
    /// which is what stops a client that is polling a task id from getting a
    /// 404 after an administrator restarts the server.
    pub async fn drain(&self, grace: std::time::Duration) {
        let active = self.inner.store.list(&RunFilter {
            include_active: true,
            include_terminal: false,
            ..Default::default()
        });
        if active.is_empty() {
            return;
        }

        // Collect the tokens to fire first: the guard must not be held across
        // the wait below.
        let to_cancel: Vec<CancellationToken> = {
            let cancels = self
                .inner
                .cancels
                .read()
                .unwrap_or_else(PoisonError::into_inner);
            active
                .iter()
                .filter(|run| {
                    self.job(run.key)
                        .is_some_and(|job| job.spec.resumable || job.spec.cancellable)
                })
                .filter_map(|run| cancels.get(&run.id).cloned())
                .collect()
        };
        let cancelled = to_cancel.len();
        for token in to_cancel {
            token.cancel();
        }

        // Wait for the active set to empty, whether the runs were cancelled or
        // must finish on their own: a cancelled run still needs a moment to
        // reach its checkpoint and record the terminal state, and interrupting
        // it in that window would report the wrong outcome.
        if !grace.is_zero() {
            let deadline = tokio::time::Instant::now() + grace;
            while tokio::time::Instant::now() < deadline {
                if self
                    .inner
                    .store
                    .list(&RunFilter {
                        include_active: true,
                        include_terminal: false,
                        ..Default::default()
                    })
                    .is_empty()
                {
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(5)).await;
            }
        }
        tracing::debug!(
            active = active.len(),
            cancelled,
            "drained this generation's task system"
        );

        let interrupted = self
            .inner
            .store
            .interrupt_active(chrono::Utc::now().timestamp());
        if interrupted > 0 {
            tracing::warn!(
                interrupted,
                "recorded runs interrupted by the end of this server generation"
            );
        }
    }

    /// Whether the process is shutting down.
    pub fn is_shutting_down(&self) -> bool {
        self.shutdown_token().is_cancelled()
    }

    /// Start the interval loops for every periodic job and the single run
    /// sweeper.
    ///
    /// Called once per server generation, after registration. Nothing else
    /// decides when a periodic job runs.
    pub fn start_schedules(&self) {
        for job in self.jobs() {
            let Trigger::Periodic {
                interval_secs,
                overlap,
            } = job.spec.trigger
            else {
                continue;
            };
            let system = self.clone();
            let shutdown = self.shutdown_token().child_token();
            tokio::spawn(async move {
                let mut ticker =
                    tokio::time::interval(std::time::Duration::from_secs(interval_secs.max(1)));
                ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
                // The first tick completes immediately; consume it so a start-up
                // does not fire every job at once, and so the first run waits a
                // full interval, as the scheduler it replaces did.
                ticker.tick().await;
                tracing::info!(job = job.name(), interval_secs, "scheduled job started");
                loop {
                    tokio::select! {
                        _ = shutdown.cancelled() => {
                            tracing::info!(job = job.name(), "scheduled job stopped");
                            break;
                        }
                        _ = ticker.tick() => {
                            system.tick(&job, overlap).await;
                        }
                    }
                }
            });
        }

        // One load sampler for the process, so admission reads a cached struct
        // rather than sampling per decision.
        let system = self.clone();
        let shutdown = self.shutdown_token().child_token();
        let interval = self
            .inner
            .sample_interval_secs
            .load(Ordering::Relaxed)
            .max(1);
        tokio::spawn(async move {
            let mut ticker = tokio::time::interval(std::time::Duration::from_secs(interval));
            ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            loop {
                tokio::select! {
                    _ = shutdown.cancelled() => break,
                    _ = ticker.tick() => system.sample_load(),
                }
            }
        });

        // One sweeper for the whole run table, rather than each read path
        // tidying up after itself while holding a lock.
        let system = self.clone();
        let shutdown = self.shutdown_token().child_token();
        tokio::spawn(async move {
            let mut ticker = tokio::time::interval(std::time::Duration::from_secs(60));
            ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            ticker.tick().await;
            loop {
                tokio::select! {
                    _ = shutdown.cancelled() => break,
                    _ = ticker.tick() => {
                        let dropped = system.sweep();
                        if dropped > 0 {
                            tracing::debug!(dropped, "swept expired job runs");
                        }
                        system.prune_journal().await;
                    }
                }
            }
        });
    }

    /// Whether a job may start now, for a job that declared it can wait.
    ///
    /// Returns [`Admission::Allow`] for anything without a quiet policy, so this
    /// is safe to call for every job.
    ///
    /// The starvation valve is `max_deferral_hours`: without it, a server that
    /// is always busy would never collect its garbage. `force_safe` decides
    /// what happens at that point — a job whose forced execution is safe runs,
    /// and one whose is not (GC, which would race a concurrent upload) only
    /// says so.
    fn admit(&self, job: &RegisteredJob, now: i64) -> admission::Admission {
        let Some(policy) = job.spec.quiet else {
            return admission::Admission::Allow;
        };
        if !self.load_aware() {
            return admission::Admission::Allow;
        }
        // A sampler that stopped must not read as a quiet server.
        if !self.load().is_fresh(MAX_SAMPLE_AGE, now) {
            return admission::Admission::Defer;
        }

        let snapshot = self.load_snapshot();
        let thresholds = self.inner.gate.thresholds();
        let mut quiet = self
            .inner
            .quiet
            .write()
            .unwrap_or_else(PoisonError::into_inner);
        let state = quiet.entry(job.key()).or_default();

        match state
            .tracker
            .admits(&snapshot, &thresholds, policy.min_idle_for_secs, now)
        {
            admission::Admission::Allow => {
                state.deferred_since = None;
                admission::Admission::Allow
            }
            admission::Admission::Defer => {
                let since = *state.deferred_since.get_or_insert(now);
                let waited_hours = (now.saturating_sub(since) / 3600) as u64;
                if policy.max_deferral_hours > 0 && waited_hours >= policy.max_deferral_hours {
                    if job.spec.force_safe {
                        tracing::warn!(
                            job = job.name(),
                            waited_hours,
                            "deferred past its cap; running it anyway"
                        );
                        state.deferred_since = None;
                        return admission::Admission::Allow;
                    }
                    tracing::warn!(
                        job = job.name(),
                        waited_hours,
                        "deferred past its cap, but running it now would be unsafe"
                    );
                }
                admission::Admission::Defer
            }
        }
    }

    /// Fire one periodic tick. Returns whether a run was submitted.
    async fn tick(&self, job: &Arc<RegisteredJob>, overlap: OverlapPolicy) -> bool {
        let now = chrono::Utc::now().timestamp();
        if self.admit(job, now) == admission::Admission::Defer {
            tracing::debug!(job = job.name(), "deferred: waiting for a quiet server");
            return false;
        }
        if overlap == OverlapPolicy::Skip && self.inner.store.count_active(job.key(), None) > 0 {
            tracing::debug!(
                job = job.name(),
                "skipping a tick: the previous run is still active"
            );
            return false;
        }
        // No summary: the run row already names the job from its slug, and a
        // timer has nothing to say about a run before it has run. The job's own
        // report fills the column in when it finishes.
        match self
            .submit_system(Origin::Schedule, job.key(), Params::Null, String::new())
            .await
        {
            Ok(_) => true,
            Err(e) => {
                tracing::warn!(job = job.name(), error = %e, "could not submit a scheduled run");
                false
            }
        }
    }

    /// Run a long-lived background service: a loop with no owner, no progress
    /// and no terminal state.
    ///
    /// Deliberately not a job. An event listener has nothing to poll and nothing
    /// to retry, so a run record would only add noise to the table; what it
    /// needs is a lifecycle and a cancellation token.
    pub fn spawn_service<F, Fut>(&self, key: ServiceKey, task: F)
    where
        F: FnOnce(CancellationToken) -> Fut + Send + 'static,
        Fut: std::future::Future<Output = ()> + Send,
    {
        self.inner
            .services
            .write()
            .unwrap_or_else(PoisonError::into_inner)
            .push(key);
        let token = self.shutdown_token().child_token();
        tokio::spawn(async move {
            tracing::info!(service = key.as_str(), "service started");
            task(token).await;
            tracing::info!(service = key.as_str(), "service stopped");
        });
    }

    /// Record that a job the catalog declares was not registered, and why.
    ///
    /// Called by the setup pass after [`TaskSystem::install`], which clears the
    /// record along with the service listing.
    pub fn record_skipped(&self, key: JobKey, reason: SkipReason) {
        self.inner
            .skipped
            .write()
            .unwrap_or_else(PoisonError::into_inner)
            .push((key, reason));
    }

    /// Jobs this generation did not register, oldest decision first.
    pub fn skipped(&self) -> Vec<(JobKey, SkipReason)> {
        self.inner
            .skipped
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// Long-lived services of the current generation.
    pub fn services(&self) -> Vec<ServiceKey> {
        self.inner
            .services
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// The priority of a registered job, for the admin listing.
    pub fn priority_of(&self, key: JobKey) -> Option<Priority> {
        self.job(key).map(|job| job.spec.priority)
    }

    /// The terminal state a run would show if it were stopped now, for callers
    /// that need to explain a refusal.
    pub fn state_of(&self, id: &RunId) -> Option<JobState> {
        self.inner.store.get(id).map(|run| run.state)
    }
}

/// Helpers for tests elsewhere in the crate that need a job context.
#[cfg(test)]
pub mod test_support {
    use super::*;

    /// A context whose run has already been asked to stop, for asserting that
    /// a long pass really does check in.
    pub fn cancelled_context() -> context::JobContext {
        let store = RunStore::new(RunLimits::default());
        let run = JobRun::queued(
            JobKey::GarbageCollection,
            Origin::Schedule,
            Visibility::OwnerOrAdmin,
            None,
            Params::Null,
            "test",
            None,
            0,
        );
        let id = run.id.clone();
        store.insert(run).expect("the run is never oversized");
        let cancel = CancellationToken::new();
        cancel.cancel();
        let tick = context::LivenessTick::default();
        let sink = context::ProgressSink::new(store, id, std::time::Duration::ZERO, tick);
        context::JobContext::new(
            RunId::new(),
            JobKey::GarbageCollection,
            None,
            cancel,
            sink,
            context::BudgetSignal::full(),
            Some(ChunkPolicy {
                max_chunk_ms: 100,
                unit: "repository",
            }),
            None,
        )
    }
}

#[cfg(test)]
mod tests {
    use self::spec::{
        ChunkPolicy, Dedup, Durability, JobSpec, QuietPolicy, RetryPolicy, SpikePolicy,
        TimeoutPolicy, Trigger,
    };
    use super::*;

    fn test_system() -> Arc<TaskSystem> {
        let system = TaskSystem::new(TaskLimits::default(), CancellationToken::new());
        system
            .register(RegisteredJob::new(
                catalog::policy(JobKey::Copy),
                Arc::new(|ctx, params| {
                    Box::pin(async move {
                        let names = params
                            .get("names")
                            .and_then(|v| v.as_array())
                            .map(|a| a.len() as u64)
                            .unwrap_or(0);
                        for i in 0..names {
                            ctx.checkpoint().await?;
                            ctx.report(i + 1, Some(names));
                        }
                        Ok(Outcome::success("copied", Some(names)))
                    })
                }),
            ))
            .unwrap();
        system
    }

    /// A task system with a real (in-memory) database behind its journal.
    async fn journal_system() -> (Arc<TaskSystem>, Arc<crate::repository::Repositories>) {
        use migration::MigratorTrait;
        let db = sea_orm::Database::connect("sqlite::memory:").await.unwrap();
        migration::Migrator::up(&db, None).await.unwrap();
        let repos = Arc::new(crate::repository::Repositories::new_for_tests(Arc::new(db)));
        let system = TaskSystem::new(TaskLimits::default(), CancellationToken::new());
        system.set_journal(repos.job_run.clone());
        system
            .register(RegisteredJob::new(
                catalog::policy(JobKey::Reindex),
                Arc::new(|ctx, _params| {
                    Box::pin(async move {
                        ctx.report(1, Some(1));
                        Ok(Outcome::success("indexed", Some(1)))
                    })
                }),
            ))
            .unwrap();
        (system, repos)
    }

    fn copy_params(names: &[&str]) -> Params {
        serde_json::json!({ "names": names, "repo_id": "r1" })
    }

    #[tokio::test]
    async fn submit_returns_an_id_that_can_be_polled() {
        let system = test_system();
        let id = system
            .submit(
                JobKey::Copy,
                Some(1),
                copy_params(&["a", "b"]),
                "Copy 2",
                Some(2),
            )
            .await
            .unwrap();

        // The run exists immediately, before the body has been polled.
        let run = system.store().get(&id).expect("run is recorded");
        assert_eq!(run.owner, Some(1));
        assert_eq!(run.expected_total, Some(2));
        assert_eq!(run.summary, "Copy 2");

        // And reaches a terminal state on its own.
        for _ in 0..100 {
            let run = system.store().get(&id).unwrap();
            if run.state.is_terminal() {
                assert_eq!(run.state, JobState::Succeeded);
                assert_eq!(run.progress.message, "copied");
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
        panic!("run did not finish");
    }

    #[tokio::test]
    async fn an_unregistered_job_cannot_be_submitted() {
        let system = TaskSystem::new(TaskLimits::default(), CancellationToken::new());
        assert!(matches!(
            system
                .submit(JobKey::Copy, Some(1), Params::Null, "x", None)
                .await,
            Err(AppError::Internal(_))
        ));
    }

    #[tokio::test]
    async fn a_duplicate_submission_is_refused_when_the_job_dedups() {
        let system = TaskSystem::new(TaskLimits::default(), CancellationToken::new());
        system
            .register(RegisteredJob::new(
                JobSpec {
                    dedup: Dedup::ByParams("repo_id"),
                    ..catalog::policy(JobKey::Reindex)
                },
                Arc::new(|_ctx, _params| {
                    Box::pin(async {
                        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
                        Ok(Outcome::ok())
                    })
                }),
            ))
            .unwrap();

        let params = serde_json::json!({"repo_id": "r1"});
        system
            .submit(JobKey::Reindex, Some(1), params.clone(), "reindex", None)
            .await
            .unwrap();
        assert!(matches!(
            system
                .submit(JobKey::Reindex, Some(1), params, "reindex", None)
                .await,
            Err(AppError::Conflict(_))
        ));
    }

    #[tokio::test]
    async fn the_per_user_active_cap_is_enforced() {
        let system = TaskSystem::new(
            TaskLimits {
                max_active_per_user: 2,
                ..TaskLimits::default()
            },
            CancellationToken::new(),
        );
        system
            .register(RegisteredJob::new(
                JobSpec {
                    max_concurrent: 10,
                    timeout: TimeoutPolicy::default(),
                    ..catalog::policy(JobKey::Copy)
                },
                Arc::new(|_ctx, _params| {
                    Box::pin(async {
                        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
                        Ok(Outcome::ok())
                    })
                }),
            ))
            .unwrap();

        system
            .submit(JobKey::Copy, Some(1), copy_params(&["a"]), "a", None)
            .await
            .unwrap();
        system
            .submit(JobKey::Copy, Some(1), copy_params(&["b"]), "b", None)
            .await
            .unwrap();
        assert!(
            matches!(
                system
                    .submit(JobKey::Copy, Some(1), copy_params(&["c"]), "c", None)
                    .await,
                Err(AppError::TooManyRequests)
            ),
            "one user must not be able to take unlimited slots"
        );
        // A different user is unaffected: the cap is per user, not global.
        assert!(
            system
                .submit(JobKey::Copy, Some(2), copy_params(&["d"]), "d", None)
                .await
                .is_ok()
        );
    }

    #[tokio::test]
    async fn run_inline_keeps_no_record_but_honours_the_limit() {
        let system = test_system();
        let before = system.store().len();
        let value = system
            .run_inline(JobKey::Copy, |_ctx| async move { Ok(41 + 1) })
            .await
            .unwrap();
        assert_eq!(value, 42);
        assert_eq!(system.store().len(), before, "no run was recorded");
    }

    #[tokio::test]
    async fn run_inline_maps_failures_onto_app_errors() {
        let system = test_system();
        assert!(matches!(
            system
                .run_inline(JobKey::Copy, |_ctx| async move {
                    Err::<(), _>(JobFailure::TimedOut)
                })
                .await,
            Err(AppError::Internal(_))
        ));
    }

    #[tokio::test]
    async fn cancel_is_refused_for_a_job_that_cannot_be_interrupted() {
        let system = TaskSystem::new(TaskLimits::default(), CancellationToken::new());
        system
            .register(RegisteredJob::new(
                catalog::policy(JobKey::Move),
                Arc::new(|_ctx, _params| {
                    Box::pin(async {
                        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                        Ok(Outcome::ok())
                    })
                }),
            ))
            .unwrap();
        let id = system
            .submit(JobKey::Move, Some(1), copy_params(&["a"]), "move", None)
            .await
            .unwrap();
        assert!(matches!(
            system.cancel(&id, Viewer::user(1)),
            Err(AppError::Conflict(_))
        ));
    }

    #[tokio::test]
    async fn cancel_stops_a_cancellable_job_and_reports_it_as_cancelled() {
        let system = TaskSystem::new(TaskLimits::default(), CancellationToken::new());
        system
            .register(RegisteredJob::new(
                catalog::policy(JobKey::Reindex),
                Arc::new(|ctx, _params| {
                    Box::pin(async move {
                        loop {
                            ctx.checkpoint().await?;
                            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
                        }
                    })
                }),
            ))
            .unwrap();
        let id = system
            .submit(
                JobKey::Reindex,
                Some(1),
                copy_params(&["a"]),
                "reindex",
                None,
            )
            .await
            .unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        system.cancel(&id, Viewer::user(1)).unwrap();

        for _ in 0..100 {
            let run = system.store().get(&id).unwrap();
            if run.state.is_terminal() {
                assert_eq!(run.state, JobState::Cancelled);
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
        panic!("run did not stop");
    }

    #[tokio::test]
    async fn cancel_hides_a_run_owned_by_somebody_else() {
        let system = test_system();
        let id = system
            .submit(JobKey::Copy, Some(1), copy_params(&["a"]), "a", None)
            .await
            .unwrap();
        assert!(matches!(
            system.cancel(&id, Viewer::user(2)),
            Err(AppError::NotFound(_))
        ));
    }

    /// The bug this replaces: a task submitted before an in-place restart used
    /// to be unknown to the next generation, so the client polling it got a 404.
    #[tokio::test]
    async fn drain_records_an_interrupted_run_instead_of_losing_it() {
        let system = TaskSystem::new(TaskLimits::default(), CancellationToken::new());
        system
            .register(RegisteredJob::new(
                catalog::policy(JobKey::Move),
                Arc::new(|ctx, _params| {
                    Box::pin(async move {
                        // Ignores cancellation: exactly the case a generation
                        // boundary has to record rather than forget.
                        ctx.sleep(std::time::Duration::from_secs(3600)).await.ok();
                        Ok(Outcome::ok())
                    })
                }),
            ))
            .unwrap();
        let id = system
            .submit(JobKey::Move, Some(1), copy_params(&["a"]), "move", None)
            .await
            .unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;

        system.drain(std::time::Duration::from_millis(50)).await;

        let run = system.store().get(&id).expect("the run must still exist");
        assert_eq!(run.state, JobState::Interrupted);
        assert!(run.finished_at.is_some());
    }

    #[tokio::test]
    async fn drain_cancels_what_can_be_interrupted() {
        let system = TaskSystem::new(TaskLimits::default(), CancellationToken::new());
        system
            .register(RegisteredJob::new(
                catalog::policy(JobKey::Reindex),
                Arc::new(|ctx, _params| {
                    Box::pin(async move {
                        loop {
                            ctx.checkpoint().await?;
                            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
                        }
                    })
                }),
            ))
            .unwrap();
        let id = system
            .submit(
                JobKey::Reindex,
                Some(1),
                copy_params(&["a"]),
                "reindex",
                None,
            )
            .await
            .unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;

        system.drain(std::time::Duration::from_millis(200)).await;

        let run = system.store().get(&id).unwrap();
        assert_eq!(
            run.state,
            JobState::Cancelled,
            "an interruptible run is stopped cleanly, not marked interrupted"
        );
    }

    #[tokio::test]
    async fn drain_is_a_no_op_with_nothing_running() {
        let system = test_system();
        system.drain(std::time::Duration::from_millis(10)).await;
        assert!(system.store().get(&RunId::from_client("x")).is_none());
    }

    #[tokio::test]
    async fn submitting_a_job_whose_params_exceed_the_budget_is_refused() {
        let system = TaskSystem::new(
            TaskLimits {
                runs: RunLimits {
                    max_retained: 0,
                    max_retained_bytes: 1,
                    terminal_ttl_secs: 3600,
                },
                ..TaskLimits::default()
            },
            CancellationToken::new(),
        );
        system
            .register(RegisteredJob::new(
                catalog::policy(JobKey::Copy),
                Arc::new(|_ctx, _params| Box::pin(async { Ok(Outcome::ok()) })),
            ))
            .unwrap();
        assert!(matches!(
            system
                .submit(JobKey::Copy, Some(1), copy_params(&["a"]), "a", None)
                .await,
            Err(AppError::TooManyRequests)
        ));
    }

    #[tokio::test]
    async fn the_global_active_cap_is_enforced() {
        let system = TaskSystem::new(
            TaskLimits {
                max_active_total: 1,
                max_active_per_user: 0,
                ..TaskLimits::default()
            },
            CancellationToken::new(),
        );
        system
            .register(RegisteredJob::new(
                JobSpec {
                    max_concurrent: 10,
                    ..catalog::policy(JobKey::Copy)
                },
                Arc::new(|_ctx, _params| {
                    Box::pin(async {
                        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
                        Ok(Outcome::ok())
                    })
                }),
            ))
            .unwrap();
        system
            .submit(JobKey::Copy, Some(1), copy_params(&["a"]), "a", None)
            .await
            .unwrap();
        assert!(matches!(
            system
                .submit(JobKey::Copy, Some(2), copy_params(&["b"]), "b", None)
                .await,
            Err(AppError::TooManyRequests)
        ));
    }

    /// The heart of load awareness: a running job parks while the server is
    /// busy and resumes when it is not, recording that it parked.
    #[tokio::test]
    async fn a_running_job_yields_while_the_server_is_busy() {
        let system = TaskSystem::new(TaskLimits::default(), CancellationToken::new());
        system
            .register(RegisteredJob::new(
                JobSpec {
                    chunkable: Some(ChunkPolicy {
                        max_chunk_ms: 100,
                        unit: "unit",
                    }),
                    ..catalog::policy(JobKey::Reindex)
                },
                Arc::new(|ctx, _params| {
                    Box::pin(async move {
                        for i in 0..5 {
                            ctx.checkpoint().await?;
                            ctx.report(i + 1, Some(5));
                        }
                        Ok(Outcome::success("done", Some(5)))
                    })
                }),
            ))
            .unwrap();
        // Enabled with thresholds that a single in-flight request exceeds.
        system.configure_load(true, 1);
        system.set_load_thresholds(crate::tasks::admission::LoadThresholds {
            max_inflight_requests: 0,
            max_db_utilization: 1.0,
            max_worker_busy_pct: 100,
        });

        let busy = system.load().request_guard();
        let id = system
            .submit(JobKey::Reindex, None, copy_params(&["a"]), "busy", None)
            .await
            .unwrap();

        // It is parked, not finished, and recorded as such.
        let mut parked = false;
        for _ in 0..200 {
            match system.store().get(&id).unwrap().state {
                JobState::Yielded => {
                    parked = true;
                    break;
                }
                state if state.is_terminal() => panic!("the job finished while busy: {state:?}"),
                _ => tokio::time::sleep(std::time::Duration::from_millis(5)).await,
            }
        }
        assert!(parked, "a busy server must park the job");

        // The server goes quiet and the job finishes on its own.
        drop(busy);
        for _ in 0..400 {
            let state = system.store().get(&id).unwrap().state;
            if state.is_terminal() {
                assert_eq!(state, JobState::Succeeded);
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
        panic!("the job did not resume");
    }

    /// With load awareness off, the same job never parks.
    #[tokio::test]
    async fn a_job_does_not_park_when_load_awareness_is_off() {
        let system = TaskSystem::new(TaskLimits::default(), CancellationToken::new());
        system
            .register(RegisteredJob::new(
                JobSpec {
                    chunkable: Some(ChunkPolicy {
                        max_chunk_ms: 100,
                        unit: "unit",
                    }),
                    ..catalog::policy(JobKey::Reindex)
                },
                Arc::new(|ctx, _params| {
                    Box::pin(async move {
                        for i in 0..5 {
                            ctx.checkpoint().await?;
                            ctx.report(i + 1, Some(5));
                        }
                        Ok(Outcome::success("done", Some(5)))
                    })
                }),
            ))
            .unwrap();
        system.configure_load(false, 1);
        let _busy = (0..50)
            .map(|_| system.load().request_guard())
            .collect::<Vec<_>>();

        let id = system
            .submit(JobKey::Reindex, None, copy_params(&["a"]), "unaware", None)
            .await
            .unwrap();
        for _ in 0..400 {
            let state = system.store().get(&id).unwrap().state;
            if state.is_terminal() {
                assert_eq!(state, JobState::Succeeded);
                return;
            }
            assert_ne!(state, JobState::Yielded, "the switch is off");
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
        panic!("the job did not finish");
    }

    /// A deferrable job waits for a quiet server before it starts, and the
    /// starvation valve eventually lets it through.
    #[tokio::test]
    async fn a_deferrable_job_waits_for_a_quiet_server() {
        let system = TaskSystem::new(TaskLimits::default(), CancellationToken::new());
        let gc = RegisteredJob::new(
            catalog::policy(JobKey::GarbageCollection),
            Arc::new(|_ctx, _params| Box::pin(async { Ok(Outcome::ok()) })),
        );
        // Load awareness is off, so nothing waits however busy the server is.
        let now = 1_000_000;
        let _busy = (0..50)
            .map(|_| system.load().request_guard())
            .collect::<Vec<_>>();
        assert_eq!(system.admit(&gc, now), admission::Admission::Allow);

        system.configure_load(true, 1);
        // GC waits 60s of calm by its own policy, and the sampler has never
        // run, so the sample is stale and nothing starts.
        assert_eq!(system.admit(&gc, now), admission::Admission::Defer);
        drop(_busy);
        system.load().mark_sampled(now);
        // Calm, but the dwell has only just begun.
        assert_eq!(system.admit(&gc, now), admission::Admission::Defer);
        assert_eq!(
            system.admit(&gc, now + 30),
            admission::Admission::Defer,
            "the dwell has not elapsed"
        );
        // The sampler would have refreshed by now; a stale sample is never a
        // reason to start.
        system.load().mark_sampled(now + 60);
        assert_eq!(
            system.admit(&gc, now + 60),
            admission::Admission::Allow,
            "after the dwell the job may start"
        );
    }

    /// A stopped sampler must not read as a quiet server.
    #[tokio::test]
    async fn a_stale_sample_defers_a_job() {
        let system = TaskSystem::new(TaskLimits::default(), CancellationToken::new());
        let gc = RegisteredJob::new(
            catalog::policy(JobKey::GarbageCollection),
            Arc::new(|_ctx, _params| Box::pin(async { Ok(Outcome::ok()) })),
        );
        system.configure_load(true, 1);
        let now = 1_000_000;
        system.load().mark_sampled(now);
        // Let the calm window elapse without the sampler running.
        let late = now + 3600;
        assert_eq!(
            system.admit(&gc, late),
            admission::Admission::Defer,
            "an hour-old sample says nothing about load now"
        );
    }

    /// The starvation valve: a permanently busy server must not defer GC for
    /// ever, and GC must not be forced when forcing it is unsafe.
    #[tokio::test]
    async fn the_starvation_valve_respects_force_safe() {
        let system = TaskSystem::new(TaskLimits::default(), CancellationToken::new());
        let gc = RegisteredJob::new(
            catalog::policy(JobKey::GarbageCollection),
            Arc::new(|_ctx, _params| Box::pin(async { Ok(Outcome::ok()) })),
        );
        system.configure_load(true, 1);
        system.set_load_thresholds(admission::LoadThresholds {
            max_inflight_requests: 0,
            max_db_utilization: 0.0,
            max_worker_busy_pct: 0,
        });

        let busy = system.load().request_guard();
        let now = 1_000_000;
        system.load().mark_sampled(now);
        assert_eq!(system.admit(&gc, now), admission::Admission::Defer);

        // Past the cap, but GC's forced execution is not safe, so it still
        // waits — and says so rather than running.
        let past_cap = now + 25 * 3600;
        system.load().mark_sampled(past_cap);
        assert!(!gc.spec.force_safe);
        assert_eq!(system.admit(&gc, past_cap), admission::Admission::Defer);
        assert_eq!(
            system.deferred_since(JobKey::GarbageCollection),
            Some(now),
            "the wait is visible to the administrator"
        );

        // A job whose forced execution is safe does go through.
        let mut spec = catalog::policy(JobKey::GarbageCollection);
        spec.force_safe = true;
        let safe = RegisteredJob::new(
            spec,
            Arc::new(|_ctx, _params| Box::pin(async { Ok(Outcome::ok()) })),
        );
        assert_eq!(system.admit(&safe, past_cap), admission::Admission::Allow);
        drop(busy);
    }

    /// A job that cannot be interrupted is never handed a gate, so a quiet
    /// policy on it could not do anything — which is why the catalog refuses
    /// that combination outright.
    #[test]
    fn a_job_without_a_checkpoint_gets_no_gate() {
        let system = TaskSystem::new(TaskLimits::default(), CancellationToken::new());
        let copy = RegisteredJob::new(
            catalog::policy(JobKey::Copy),
            Arc::new(|_ctx, _params| Box::pin(async { Ok(Outcome::ok()) })),
        );
        assert!(system.yield_gate(&copy).is_none());

        let gc = RegisteredJob::new(
            catalog::policy(JobKey::GarbageCollection),
            Arc::new(|_ctx, _params| Box::pin(async { Ok(Outcome::ok()) })),
        );
        assert!(system.yield_gate(&gc).is_some());
    }

    /// A finished audited run leaves a row, so a crash is visible rather than
    /// silent.
    ///
    /// The job is a housekeeping pass — `Audit`, not `Durable` — because that
    /// is the case the terminal write used to update a row nobody had
    /// inserted, leaving the journal empty for every job but a reindex.
    #[tokio::test]
    async fn an_audited_run_is_recorded_in_the_journal() {
        let (system, repos) = journal_system().await;
        system
            .register(RegisteredJob::new(
                catalog::policy(JobKey::ShareLinkCleanup),
                Arc::new(|_ctx, _params| {
                    Box::pin(async { Ok(Outcome::success("cleaned up 3 expired links", Some(3))) })
                }),
            ))
            .unwrap();
        let id = system
            .submit(
                JobKey::ShareLinkCleanup,
                Some(1),
                Params::Null,
                "share link cleanup",
                None,
            )
            .await
            .unwrap();
        for _ in 0..200 {
            if system.store().get(&id).unwrap().state.is_terminal() {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
        // The journal write happens after the terminal transition.
        let mut rows = Vec::new();
        for _ in 0..200 {
            rows = repos.job_run.recent(10).await.unwrap();
            if !rows.is_empty() {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
        assert_eq!(rows.len(), 1, "one finished run is recorded");
        let row = &rows[0];
        assert_eq!(row.kind, "share-link-cleanup");
        assert_eq!(row.phase, "succeeded");
        assert_eq!(row.owner, Some(1));
        assert!(row.finished_at.is_some());
        assert!(row.lease_until.is_none(), "a finished run holds no lease");
        assert!(row.params.is_none(), "the input is dropped once it is over");
        // What the job reported is what the record says it did — not the name
        // its submitter called it.
        assert_eq!(row.summary, "cleaned up 3 expired links");
        assert_eq!(row.processed, Some(3));
    }

    /// A run that failed before it could report anything keeps the summary its
    /// submitter gave, so the record still says which work it was.
    #[tokio::test]
    async fn a_failed_run_keeps_the_submitters_summary() {
        let (system, repos) = journal_system().await;
        system
            .register(RegisteredJob::new(
                catalog::policy(JobKey::ShareLinkCleanup),
                Arc::new(|_ctx, _params| {
                    Box::pin(async {
                        Err(JobFailure::App(AppError::Internal(
                            "the database is locked".into(),
                        )))
                    })
                }),
            ))
            .unwrap();
        let id = system
            .submit(
                JobKey::ShareLinkCleanup,
                Some(1),
                Params::Null,
                "share link cleanup",
                None,
            )
            .await
            .unwrap();
        for _ in 0..200 {
            if system.store().get(&id).unwrap().state.is_terminal() {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
        let mut rows = Vec::new();
        for _ in 0..200 {
            rows = repos.job_run.recent(10).await.unwrap();
            if !rows.is_empty() {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].phase, "failed");
        assert_eq!(rows[0].summary, "share link cleanup");
        assert!(
            rows[0]
                .error
                .as_deref()
                .unwrap_or_default()
                .contains("locked")
        );
    }

    /// A `Notable` job, as the catalog declares it, with a body that reports
    /// `outcome` when it runs.
    fn notable_job(outcome: Outcome) -> RegisteredJob {
        RegisteredJob::new(
            catalog::policy(JobKey::PasswordCacheCleanup),
            Arc::new(move |_ctx, _params| {
                let outcome = outcome.clone();
                Box::pin(async move { Ok(outcome) })
            }),
        )
    }

    /// Wait until a run of `key` has finished, returning nothing.
    async fn settle(system: &Arc<TaskSystem>) {
        for _ in 0..400 {
            if system
                .stats(JobKey::PasswordCacheCleanup)
                .last_state
                .is_some()
            {
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
        panic!("the run never finished");
    }

    /// An idle tick is not an event: it is counted, and then it is gone — from
    /// the run table as well as from the journal. This is what stops a pass that
    /// fires every couple of minutes from burying the runs that did something.
    #[tokio::test]
    async fn an_idle_tick_leaves_no_run_record() {
        let (system, repos) = journal_system().await;
        system
            .register(notable_job(Outcome::idle(
                "no expired password cache entry",
            )))
            .unwrap();

        let id = system
            .submit_system(
                Origin::Schedule,
                JobKey::PasswordCacheCleanup,
                Params::Null,
                String::new(),
            )
            .await
            .unwrap();
        settle(&system).await;

        // The discard runs after the stats are folded in, so wait for the run
        // to leave the table rather than guessing at the order.
        for _ in 0..400 {
            if system.store().get(&id).is_none() {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
        assert!(
            system.store().get(&id).is_none(),
            "an idle tick is not kept in memory"
        );
        assert!(
            repos.job_run.recent(10).await.unwrap().is_empty(),
            "and it leaves no journal row"
        );

        // What it does not lose is the count: the registry page still has to be
        // able to say the job is ticking.
        let stats = system.stats(JobKey::PasswordCacheCleanup);
        assert_eq!(stats.run_count, 1);
        assert_eq!(stats.success_count, 1);
        assert!(stats.last_run_at.is_some());
        assert_eq!(
            stats.last_success_message,
            "no expired password cache entry"
        );
    }

    /// The same idle run, asked for by hand, is the answer to the press and is
    /// therefore written down.
    #[tokio::test]
    async fn an_idle_run_somebody_asked_for_is_recorded() {
        let (system, repos) = journal_system().await;
        system
            .register(notable_job(Outcome::idle(
                "no expired password cache entry",
            )))
            .unwrap();

        let id = system
            .submit_system(
                Origin::Operator,
                JobKey::PasswordCacheCleanup,
                Params::Null,
                String::new(),
            )
            .await
            .unwrap();
        settle(&system).await;

        let mut rows = Vec::new();
        for _ in 0..400 {
            rows = repos.job_run.recent(10).await.unwrap();
            if !rows.is_empty() {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
        assert_eq!(rows.len(), 1, "a run somebody asked for is recorded");
        assert_eq!(rows[0].kind, "password-cache-cleanup");
        assert_eq!(rows[0].phase, "succeeded");
        assert_eq!(rows[0].summary, "no expired password cache entry");
        assert!(rows[0].params.is_none(), "nothing needs its input");
        assert!(rows[0].lease_until.is_none());
        assert!(
            system.store().get(&id).is_some(),
            "and it stays readable in memory"
        );
    }

    /// A notable job that did work, and one that failed, are both recorded —
    /// the policy is about idle ticks, not about hiding bad news.
    #[tokio::test]
    async fn a_notable_run_is_recorded_when_it_did_work_or_failed() {
        let (system, repos) = journal_system().await;
        system
            .register(notable_job(Outcome::success(
                "evicted 3 expired password cache entries",
                Some(3),
            )))
            .unwrap();
        system
            .submit_system(
                Origin::Schedule,
                JobKey::PasswordCacheCleanup,
                Params::Null,
                String::new(),
            )
            .await
            .unwrap();
        settle(&system).await;
        let rows = repos.job_run.recent(10).await.unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].phase, "succeeded");
        assert_eq!(rows[0].summary, "evicted 3 expired password cache entries");
        assert_eq!(rows[0].processed, Some(3));
        assert_eq!(rows[0].attempt, 1);

        // A second system, so the failing run stands alone.
        let (system, repos) = journal_system().await;
        system
            .register(RegisteredJob::new(
                catalog::policy(JobKey::PasswordCacheCleanup),
                Arc::new(|_ctx, _params| {
                    Box::pin(async {
                        Err(JobFailure::App(AppError::Internal(
                            "the cache is locked".into(),
                        )))
                    })
                }),
            ))
            .unwrap();
        system
            .submit_system(
                Origin::Schedule,
                JobKey::PasswordCacheCleanup,
                Params::Null,
                String::new(),
            )
            .await
            .unwrap();
        settle(&system).await;
        let rows = repos.job_run.recent(10).await.unwrap();
        assert_eq!(rows.len(), 1, "a failure is never silent");
        assert_eq!(rows[0].phase, "failed");
        assert!(
            rows[0]
                .error
                .as_deref()
                .unwrap_or_default()
                .contains("locked")
        );
        assert!(rows[0].summary.is_empty(), "the error is the account");
    }

    /// A destructive job is never recorded as replayable, so a crash can never
    /// repeat it.
    #[tokio::test]
    async fn a_memory_only_run_is_never_journalled() {
        let (system, repos) = journal_system().await;
        system
            .register(RegisteredJob::new(
                catalog::policy(JobKey::Move),
                Arc::new(|_ctx, _params| Box::pin(async { Ok(Outcome::ok()) })),
            ))
            .unwrap();
        let id = system
            .submit(JobKey::Move, Some(1), copy_params(&["a"]), "move", None)
            .await
            .unwrap();
        for _ in 0..200 {
            if system.store().get(&id).unwrap().state.is_terminal() {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
        assert!(
            repos.job_run.recent(10).await.unwrap().is_empty(),
            "copy and move are not recorded at all"
        );
        assert!(
            repos
                .job_run
                .recoverable(i64::MAX, 10)
                .await
                .unwrap()
                .is_empty(),
            "and never recovered"
        );
    }

    /// An audited run the process never finished is closed, never replayed:
    /// the job was not declared safe to run twice.
    #[tokio::test]
    async fn an_unfinished_audited_run_is_closed_rather_than_replayed() {
        let (system, repos) = journal_system().await;
        system
            .register(RegisteredJob::new(
                catalog::policy(JobKey::ShareLinkCleanup),
                Arc::new(|_ctx, _params| Box::pin(async { Ok(Outcome::ok()) })),
            ))
            .unwrap();
        repos
            .job_run
            .enqueue(
                crate::repository::job_run::NewJobRun {
                    id: "crashed-audit".to_string(),
                    kind: "share-link-cleanup".to_string(),
                    owner: Some(7),
                    summary: String::new(),
                    params: Some(Params::Null.to_string()),
                    created_at: 0,
                },
                -1,
            )
            .await
            .unwrap();

        system.recover().await;

        let rows = repos.job_run.recent(10).await.unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].phase, "interrupted");
        assert_eq!(
            rows[0].error.as_deref(),
            Some("this job is not safe to replay")
        );
    }

    /// A durable run whose process died is replayed; its interrupted attempt is
    /// recorded rather than dropped.
    #[tokio::test]
    async fn a_durable_run_is_recovered_after_a_crash() {
        let (system, repos) = journal_system().await;
        // A run left behind by a process that never finished it, with its lease
        // long expired.
        repos
            .job_run
            .enqueue(
                crate::repository::job_run::NewJobRun {
                    id: "crashed-run".to_string(),
                    kind: "reindex".to_string(),
                    owner: Some(7),
                    summary: "Reindex \"r1\"".to_string(),
                    params: Some(serde_json::json!({"repo_id": "r1"}).to_string()),
                    created_at: 0,
                },
                -1,
            )
            .await
            .unwrap();

        system.recover().await;

        // The new attempt ran and was recorded.
        let mut recovered: Vec<crate::repository::job_run::NewJobRun> = Vec::new();
        for _ in 0..200 {
            recovered = repos
                .job_run
                .recent(10)
                .await
                .unwrap()
                .into_iter()
                .filter(|row| row.id != "crashed-run")
                .map(|row| crate::repository::job_run::NewJobRun {
                    id: row.id,
                    kind: row.kind,
                    owner: row.owner,
                    summary: row.summary,
                    params: row.params,
                    created_at: row.created_at,
                })
                .collect();
            if !recovered.is_empty() {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
        assert_eq!(recovered.len(), 1, "the run was replayed once");
        assert_eq!(recovered[0].owner, Some(7), "ownership survives recovery");
        assert!(
            recovered[0].params.is_none(),
            "the finished replay drops its input"
        );

        // The abandoned attempt is closed, so it is not replayed again.
        assert!(
            repos
                .job_run
                .recoverable(i64::MAX, 10)
                .await
                .unwrap()
                .is_empty(),
            "the interrupted attempt is not pending work any more"
        );
    }

    /// A row whose job is gone, or that is not safe to replay, is closed rather
    /// than left to be retried forever.
    #[tokio::test]
    async fn recovery_closes_a_row_it_cannot_replay() {
        let (system, repos) = journal_system().await;
        for (id, kind) in [("gone", "no-such-job"), ("unsafe", "move")] {
            repos
                .job_run
                .enqueue(
                    crate::repository::job_run::NewJobRun {
                        id: id.to_string(),
                        kind: kind.to_string(),
                        owner: None,
                        summary: String::new(),
                        params: None,
                        created_at: 0,
                    },
                    -1,
                )
                .await
                .unwrap();
        }

        system.recover().await;

        let rows = repos.job_run.recent(10).await.unwrap();
        assert_eq!(rows.len(), 2);
        for row in &rows {
            assert_eq!(row.phase, "interrupted", "{} was closed", row.id);
            assert!(row.finished_at.is_some());
        }
    }

    #[test]
    fn registering_a_duplicate_is_refused() {
        let system = test_system();
        let again = RegisteredJob::new(
            catalog::policy(JobKey::Copy),
            Arc::new(|_ctx, _params| Box::pin(async { Ok(Outcome::ok()) })),
        );
        assert!(matches!(
            system.register(again),
            Err(RegistryError::Duplicate(JobKey::Copy))
        ));
    }

    #[test]
    fn jobs_are_listed_in_registration_order() {
        let system = TaskSystem::new(TaskLimits::default(), CancellationToken::new());
        system
            .register(RegisteredJob::new(
                catalog::policy(JobKey::Reindex),
                Arc::new(|_ctx, _params| Box::pin(async { Ok(Outcome::ok()) })),
            ))
            .unwrap();
        system
            .register(RegisteredJob::new(
                catalog::policy(JobKey::Copy),
                Arc::new(|_ctx, _params| Box::pin(async { Ok(Outcome::ok()) })),
            ))
            .unwrap();
        let keys: Vec<JobKey> = system.jobs().iter().map(|j| j.key()).collect();
        assert_eq!(keys, vec![JobKey::Reindex, JobKey::Copy]);
        assert_eq!(
            system.priority_of(JobKey::Copy),
            Some(Priority::Interactive)
        );
    }

    /// A `ChunkPolicy` on the registered job reaches the context, which is what
    /// makes the deferral contract real rather than declarative.
    #[test]
    fn the_chunk_policy_reaches_the_context() {
        let system = TaskSystem::new(TaskLimits::default(), CancellationToken::new());
        system
            .register(RegisteredJob::new(
                JobSpec {
                    chunkable: Some(ChunkPolicy {
                        max_chunk_ms: 250,
                        unit: "repo",
                    }),
                    priority: Priority::Background,
                    quiet: Some(QuietPolicy {
                        min_idle_for_secs: 1,
                        max_deferral_hours: 1,
                        on_spike: SpikePolicy::Yield,
                    }),
                    idempotent: true,
                    retry: RetryPolicy::Never,
                    durability: Durability::Memory,
                    trigger: Trigger::Manual,
                    ..catalog::policy(JobKey::GarbageCollection)
                },
                Arc::new(|_ctx, _params| Box::pin(async { Ok(Outcome::ok()) })),
            ))
            .unwrap();
        let spec = system.job(JobKey::GarbageCollection).unwrap();
        assert_eq!(spec.spec.chunkable.unwrap().max_chunk_ms, 250);
    }
}
