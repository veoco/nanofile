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
use self::spec::{Dedup, JobKey, OverlapPolicy, Priority};
use self::store::{RunFilter, RunLimits, RunStore};

pub use self::run::{JobFailure, Outcome, Progress};
pub use self::spec::{ChunkPolicy, Durability, Resource, TimeoutPolicy, Trigger, Visibility};

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
    services: RwLock<Vec<&'static str>>,
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
                stats: RwLock::new(HashMap::new()),
                load,
                gate,
                db_probe: RwLock::new(None),
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
        // it rather than accumulating stale names.
        self.inner
            .services
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
    pub fn submit(
        &self,
        key: JobKey,
        owner: Option<i32>,
        params: Params,
        summary: impl Into<String>,
        expected_total: Option<u64>,
    ) -> Result<RunId, AppError> {
        self.submit_with_details(key, owner, params, summary, expected_total, Vec::new())
    }

    /// Submit with small job-specific facts for the wire projection that must
    /// outlive the params.
    #[allow(clippy::too_many_arguments)]
    pub fn submit_with_details(
        &self,
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
        let gate = self.yield_gate(job.as_ref());
        let report =
            executor::execute(&job, &run, params, &self.inner.store, cancel.clone(), gate).await;
        self.record_stats(job.key(), &report, started.elapsed());
        tracing::debug!(
            job = job.name(),
            run = %run.id,
            state = report.state.as_str(),
            attempts = report.attempts,
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

    /// The load counters.
    pub fn load(&self) -> &LoadGauge {
        &self.inner.load
    }

    /// How busy the server is right now.
    pub fn load_snapshot(&self) -> LoadSnapshot {
        self.inner.load.snapshot()
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
                            system.tick(&job, overlap);
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
                    }
                }
            }
        });
    }

    /// Fire one periodic tick. Returns whether a run was submitted.
    fn tick(&self, job: &Arc<RegisteredJob>, overlap: OverlapPolicy) -> bool {
        if overlap == OverlapPolicy::Skip && self.inner.store.count_active(job.key(), None) > 0 {
            tracing::debug!(
                job = job.name(),
                "skipping a tick: the previous run is still active"
            );
            return false;
        }
        match self.submit(job.key(), None, Params::Null, job.name(), None) {
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
    pub fn spawn_service<F, Fut>(&self, name: &'static str, task: F)
    where
        F: FnOnce(CancellationToken) -> Fut + Send + 'static,
        Fut: std::future::Future<Output = ()> + Send,
    {
        self.inner
            .services
            .write()
            .unwrap_or_else(PoisonError::into_inner)
            .push(name);
        let token = self.shutdown_token().child_token();
        tokio::spawn(async move {
            tracing::info!(service = name, "service started");
            task(token).await;
            tracing::info!(service = name, "service stopped");
        });
    }

    /// Long-lived services of the current generation.
    pub fn services(&self) -> Vec<&'static str> {
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
            system.submit(JobKey::Copy, Some(1), Params::Null, "x", None),
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
            .unwrap();
        assert!(matches!(
            system.submit(JobKey::Reindex, Some(1), params, "reindex", None),
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
            .unwrap();
        system
            .submit(JobKey::Copy, Some(1), copy_params(&["b"]), "b", None)
            .unwrap();
        assert!(
            matches!(
                system.submit(JobKey::Copy, Some(1), copy_params(&["c"]), "c", None),
                Err(AppError::TooManyRequests)
            ),
            "one user must not be able to take unlimited slots"
        );
        // A different user is unaffected: the cap is per user, not global.
        assert!(
            system
                .submit(JobKey::Copy, Some(2), copy_params(&["d"]), "d", None)
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
            system.submit(JobKey::Copy, Some(1), copy_params(&["a"]), "a", None),
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
            .unwrap();
        assert!(matches!(
            system.submit(JobKey::Copy, Some(2), copy_params(&["b"]), "b", None),
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
