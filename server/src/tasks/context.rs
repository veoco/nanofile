//! What a running job is handed: identity, progress reporting, cancellation
//! and the checkpoints that make load awareness possible.

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::time::Duration;

use tokio_util::sync::CancellationToken;

use super::admission::YieldGate;
use super::run::{JobFailure, JobState, RunId};
use super::spec::{ChunkPolicy, JobKey};
use super::store::RunStore;

/// The share of its normal work a job should attempt right now, as a
/// percentage.
///
/// Scaled by load: a job written in terms of a variable batch size or
/// concurrency width reads this and shrinks, which is the only way to reduce
/// load from a running task — tokio cannot preempt one.
#[derive(Clone)]
pub struct BudgetSignal {
    percent: Arc<AtomicU32>,
}

impl BudgetSignal {
    /// Full speed, which is the only setting before load awareness is on.
    pub fn full() -> Self {
        Self {
            percent: Arc::new(AtomicU32::new(100)),
        }
    }

    pub fn get(&self) -> f32 {
        self.percent.load(Ordering::Relaxed) as f32 / 100.0
    }

    pub fn set_percent(&self, percent: u32) {
        self.percent.store(percent.min(100), Ordering::Relaxed);
    }
}

impl Default for BudgetSignal {
    fn default() -> Self {
        Self::full()
    }
}

/// Liveness counter shared between a context and the executor's watchdog.
///
/// Bumped by every checkpoint and every report, so the watchdog measures "the
/// job is still getting on with it" rather than "its reported number changed" —
/// a job legitimately working on one large item must call `checkpoint` inside
/// it, which is exactly what [`ChunkPolicy`] asks for.
#[derive(Clone, Default)]
pub(crate) struct LivenessTick(Arc<AtomicU64>);

impl LivenessTick {
    pub(crate) fn bump(&self) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }

    pub(crate) fn get(&self) -> u64 {
        self.0.load(Ordering::SeqCst)
    }
}

/// Per-run progress writes, coalesced.
///
/// A job may report far more often than a reader needs, so writes are limited
/// to one per `min_interval`; the rest update liveness only. Everything here is
/// synchronous, so reporting never costs the job an await (and never spawns).
/// The terminal transition is written by the executor, not through here, so a
/// coalesced final report cannot lose the outcome.
#[derive(Clone)]
pub(crate) struct ProgressSink {
    store: RunStore,
    id: RunId,
    base: std::time::Instant,
    min_interval_ms: u64,
    /// Millis since `base` at the last write, used as the throttle slot.
    /// [`NEVER`] until the first write, so the first report always lands.
    last_ms: Arc<AtomicU64>,
    tick: LivenessTick,
}

impl ProgressSink {
    pub(crate) fn new(
        store: RunStore,
        id: RunId,
        min_interval: Duration,
        tick: LivenessTick,
    ) -> Self {
        let base = std::time::Instant::now();
        Self {
            store,
            id,
            base,
            min_interval_ms: min_interval.as_millis() as u64,
            last_ms: Arc::new(AtomicU64::new(NEVER)),
            tick,
        }
    }

    pub(crate) fn report(&self, done: u64, total: Option<u64>, message: Option<String>) {
        // Liveness counts even a coalesced report: the job is running, which is
        // what the watchdog asks about.
        self.tick.bump();
        if !self.claim_slot() {
            return;
        }
        self.store.update(&self.id, |run| {
            run.progress.done = done;
            if total.is_some() {
                run.progress.total = total;
            }
            if let Some(message) = message {
                run.progress.message = message;
            }
        });
    }

    /// Take the current throttle slot, if it is free.
    fn claim_slot(&self) -> bool {
        let now = self.base.elapsed().as_millis() as u64;
        let mut last = self.last_ms.load(Ordering::Relaxed);
        loop {
            let due = last == NEVER || now.saturating_sub(last) >= self.min_interval_ms;
            if !due {
                return false;
            }
            // Whoever wins the swap owns this window; the rest coalesce.
            match self.last_ms.compare_exchange_weak(
                last,
                now,
                Ordering::Relaxed,
                Ordering::Relaxed,
            ) {
                Ok(_) => return true,
                Err(actual) => last = actual,
            }
        }
    }
}

/// Sentinel for "this sink has not written yet".
const NEVER: u64 = u64::MAX;

/// The clock behind a job's declared chunk budget.
///
/// How long a chunk takes is something only the job can see, so the promise in
/// [`ChunkPolicy::max_chunk_ms`] is kept where the job checks in: a checkpoint
/// that lands more than the budget after the previous one gives the runtime
/// back. That is what stops a job whose chunk is synchronous CPU from owning
/// its worker until the loop ends — `checkpoint` is otherwise a no-op while the
/// server is calm.
///
/// Shared by every clone of the context, because the job body and the progress
/// clone can both reach it.
struct ChunkBudget {
    max_chunk_ms: u64,
    /// When the context was built, so a chunk can be measured in millis.
    base: std::time::Instant,
    /// Millis since `base` at the last yield.
    last_ms: AtomicU64,
}

/// The handle a job body uses to talk to the task system.
#[derive(Clone)]
pub struct JobContext {
    id: RunId,
    key: JobKey,
    owner: Option<i32>,
    cancel: CancellationToken,
    progress: ProgressSink,
    budget: BudgetSignal,
    chunk: Option<ChunkPolicy>,
    /// The declared chunk budget, and the clock that enforces it. `None` for a
    /// job that declared no unit — it is not asked to hand anything back.
    chunk_budget: Option<Arc<ChunkBudget>>,
    /// The load gate, for a job that declared it can be interrupted. `None`
    /// means the job never parks, which is the contract the catalog enforces.
    gate: Option<YieldGate>,
}

impl JobContext {
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        id: RunId,
        key: JobKey,
        owner: Option<i32>,
        cancel: CancellationToken,
        progress: ProgressSink,
        budget: BudgetSignal,
        chunk: Option<ChunkPolicy>,
        gate: Option<YieldGate>,
    ) -> Self {
        // A job that declared no unit, or declared one of zero, is not bounded
        // and never hands the runtime back — `0` reads as "unlimited" here as
        // it does for the other caps in the catalog.
        let chunk_budget = chunk
            .filter(|policy| policy.max_chunk_ms > 0)
            .map(|policy| {
                Arc::new(ChunkBudget {
                    max_chunk_ms: policy.max_chunk_ms,
                    base: std::time::Instant::now(),
                    last_ms: AtomicU64::new(0),
                })
            });
        Self {
            id,
            key,
            owner,
            cancel,
            progress,
            budget,
            chunk,
            chunk_budget,
            gate,
        }
    }

    pub fn id(&self) -> &RunId {
        &self.id
    }

    pub fn key(&self) -> JobKey {
        self.key
    }

    /// The submitting user, for a job that scopes its work to one.
    pub fn owner(&self) -> Option<i32> {
        self.owner
    }

    /// How much of its normal work the job should attempt now.
    ///
    /// Full speed unless load awareness is on and the server is busy, in which
    /// case it scales down. A job that reads this can slow itself; one that
    /// ignores it still parks at its checkpoints.
    pub fn budget(&self) -> f32 {
        match &self.gate {
            Some(gate) => gate.budget(),
            None => self.budget.get(),
        }
    }

    /// How long one indivisible unit of this job may run, if it declares one.
    pub fn chunk(&self) -> Option<ChunkPolicy> {
        self.chunk
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancel.is_cancelled()
    }

    pub fn cancellation(&self) -> CancellationToken {
        self.cancel.clone()
    }

    /// Report progress. Cheap and synchronous to call often; writes are
    /// coalesced to one per throttle window.
    pub fn report(&self, done: u64, total: Option<u64>) {
        self.progress.report(done, total, None);
    }

    /// Report progress together with a message.
    pub fn report_with_message(&self, done: u64, total: Option<u64>, message: impl Into<String>) {
        self.progress.report(done, total, Some(message.into()));
    }

    /// Record a small fact the client-compatibility projection needs after the
    /// run's params have been released — a reindex run's indexed and skipped
    /// counts, for instance.
    pub fn set_detail(&self, key: &str, value: serde_json::Value) {
        self.progress
            .store
            .update(&self.id, |run| run.set_detail(key, value));
    }

    /// The one interruption point a job must call.
    ///
    /// Returns [`JobFailure::Cancelled`] once the run has been asked to stop.
    /// Load-aware yielding is layered onto this same call: the checkpoint is
    /// deliberately the place a job waits, so parking needs no second API and a
    /// job cannot yield "by accident" between units.
    ///
    /// While parked the run is recorded as [`JobState::Yielded`], which is
    /// deliberately distinct from `Running`: the run is alive, and a recovery
    /// pass must not mistake it for one that died.
    ///
    /// It is also where a job's declared [`ChunkPolicy::max_chunk_ms`] is
    /// honoured: a chunk that has run its course gives the runtime back,
    /// whether or not the server is busy.
    pub async fn checkpoint(&self) -> Result<(), JobFailure> {
        self.progress.tick.bump();
        if self.cancel.is_cancelled() {
            return Err(JobFailure::Cancelled);
        }

        // Before the gate, so the hand-back happens on a calm server too —
        // which is the case the gate cannot cover.
        self.yield_after_chunk().await;

        let Some(gate) = &self.gate else {
            return Ok(());
        };
        if !gate.is_enabled() || gate.is_calm() {
            return Ok(());
        }

        let id = self.id.clone();
        self.progress.store.update(&id, |run| {
            if run.state == JobState::Running {
                run.state = JobState::Yielded;
            }
        });
        let calm = gate.wait_until_calm(&self.cancel).await;
        self.progress.store.update(&id, |run| {
            if run.state == JobState::Yielded {
                run.state = JobState::Running;
            }
        });
        if !calm || self.cancel.is_cancelled() {
            return Err(JobFailure::Cancelled);
        }
        Ok(())
    }

    /// Give the runtime back once the declared chunk has run its course.
    ///
    /// A yield, not a park: the job is not waiting for anything, it is letting
    /// the other tasks on its worker be polled. Cheap enough to be checked at
    /// every checkpoint — a job whose chunks are short yields rarely, because
    /// the budget is what decides, not the checkpoint.
    async fn yield_after_chunk(&self) {
        let Some(budget) = &self.chunk_budget else {
            return;
        };
        let now = budget.base.elapsed().as_millis() as u64;
        let mut last = budget.last_ms.load(Ordering::Relaxed);
        loop {
            if now.saturating_sub(last) < budget.max_chunk_ms {
                return;
            }
            // Whoever wins the window owns the yield: a job that works through
            // its chunks concurrently needs the runtime back once, not once per
            // task that happens to check in.
            match budget.last_ms.compare_exchange_weak(
                last,
                now,
                Ordering::Relaxed,
                Ordering::Relaxed,
            ) {
                Ok(_) => {
                    tokio::task::yield_now().await;
                    return;
                }
                Err(actual) => last = actual,
            }
        }
    }

    /// Run blocking work on the blocking pool, but only after a checkpoint.
    ///
    /// The ordering is the point: when the server is busy the job parks
    /// *before* taking a blocking thread, so a background job stops
    /// replenishing the pool instead of queueing ahead of interactive work.
    pub async fn run_blocking<F, T>(&self, f: F) -> Result<T, JobFailure>
    where
        F: FnOnce() -> T + Send + 'static,
        T: Send + 'static,
    {
        self.checkpoint().await?;
        tokio::task::spawn_blocking(f)
            .await
            .map_err(|e| JobFailure::App(base::error::AppError::internal(e.to_string())))
    }

    /// An interruptible sleep: checked on both sides, so a cancelled run does
    /// not sleep out its pause.
    pub async fn sleep(&self, duration: Duration) -> Result<(), JobFailure> {
        self.checkpoint().await?;
        tokio::time::sleep(duration).await;
        self.checkpoint().await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tasks::run::{JobRun, Origin};
    use crate::tasks::spec::Visibility;
    use crate::tasks::store::RunLimits;

    fn sink(min_interval: Duration) -> (ProgressSink, RunStore, RunId, LivenessTick) {
        let store = RunStore::new(RunLimits::default());
        let run = JobRun::queued(
            JobKey::Copy,
            Origin::Request,
            Visibility::Owner,
            Some(1),
            serde_json::json!({}),
            "t",
            None,
            0,
        );
        let id = run.id.clone();
        store.insert(run).unwrap();
        let tick = LivenessTick::default();
        (
            ProgressSink::new(store.clone(), id.clone(), min_interval, tick.clone()),
            store,
            id,
            tick,
        )
    }

    /// A context with no gate, so only its chunk budget can yield.
    fn context(chunk: Option<ChunkPolicy>) -> JobContext {
        let (sink, _store, id, _tick) = sink(Duration::ZERO);
        JobContext::new(
            id,
            JobKey::Copy,
            Some(1),
            CancellationToken::new(),
            sink,
            BudgetSignal::full(),
            chunk,
            None,
        )
    }

    /// A body whose only await points are its checkpoints, plus a witness task
    /// that can only run if one of them gives the runtime back.
    ///
    /// `#[tokio::test]` is a current-thread runtime, so a job that never yields
    /// owns the thread for the whole loop and the witness never runs.
    async fn chunked_loop(chunk: Option<ChunkPolicy>, chunk_ms: u64) -> u64 {
        let ctx = context(chunk);
        let ran = Arc::new(AtomicU64::new(0));
        tokio::spawn({
            let ran = ran.clone();
            async move {
                ran.fetch_add(1, Ordering::SeqCst);
            }
        });
        for _ in 0..4 {
            ctx.checkpoint().await.unwrap();
            std::thread::sleep(Duration::from_millis(chunk_ms));
        }
        ran.load(Ordering::SeqCst)
    }

    /// A chunk that has run its course hands the runtime back.
    #[tokio::test]
    async fn a_declared_chunk_hands_the_runtime_back() {
        let ran = chunked_loop(
            Some(ChunkPolicy {
                max_chunk_ms: 5,
                unit: "unit",
            }),
            6,
        )
        .await;
        assert!(ran > 0, "a chunk past its budget must yield");
    }

    /// A chunk still inside its budget does not: the budget decides, not the
    /// checkpoint.
    #[tokio::test]
    async fn a_chunk_inside_its_budget_does_not_yield() {
        let ran = chunked_loop(
            Some(ChunkPolicy {
                max_chunk_ms: 3_600_000,
                unit: "unit",
            }),
            1,
        )
        .await;
        assert_eq!(ran, 0, "a chunk inside its budget must not yield");
    }

    /// A job that declared no unit is not asked to hand anything back.
    #[tokio::test]
    async fn a_job_without_a_declared_chunk_does_not_yield() {
        let ran = chunked_loop(None, 6).await;
        assert_eq!(ran, 0, "an unbounded job must not yield");
    }

    #[test]
    fn reports_inside_the_window_are_coalesced() {
        let (sink, store, id, _tick) = sink(Duration::from_secs(3600));
        sink.report(1, Some(10), None);
        sink.report(2, Some(10), None);
        sink.report(3, Some(10), None);
        assert_eq!(
            store.get(&id).unwrap().progress.done,
            1,
            "only the first report lands inside the window"
        );
    }

    #[test]
    fn an_immediate_sink_writes_every_report() {
        let (sink, store, id, _tick) = sink(Duration::ZERO);
        sink.report(1, Some(10), None);
        sink.report(2, Some(10), None);
        assert_eq!(store.get(&id).unwrap().progress.done, 2);
        assert_eq!(store.get(&id).unwrap().progress.total, Some(10));
    }

    /// Liveness is bumped even by a coalesced report: the watchdog asks whether
    /// the job is running, not whether its number moved.
    #[test]
    fn liveness_counts_coalesced_reports() {
        let (sink, _store, _id, tick) = sink(Duration::from_secs(3600));
        let before = tick.get();
        sink.report(1, None, None);
        sink.report(2, None, None);
        assert_eq!(tick.get(), before + 2);
    }

    #[test]
    fn a_total_is_only_replaced_when_supplied() {
        let (sink, store, id, _tick) = sink(Duration::ZERO);
        sink.report(1, Some(4), None);
        sink.report(2, None, None);
        let progress = store.get(&id).unwrap().progress;
        assert_eq!(progress.done, 2);
        assert_eq!(
            progress.total,
            Some(4),
            "an unknown total keeps the old one"
        );
    }

    #[test]
    fn a_message_replaces_the_previous_one() {
        let (sink, store, id, _tick) = sink(Duration::ZERO);
        sink.report(1, Some(4), Some("first".into()));
        sink.report(2, Some(4), None);
        assert_eq!(store.get(&id).unwrap().progress.message, "first");
    }

    #[test]
    fn reporting_to_a_gone_run_is_harmless() {
        let (sink, store, id, _tick) = sink(Duration::ZERO);
        store.update(&id, |run| {
            run.state = crate::tasks::run::JobState::Succeeded;
        });
        // The run still exists; a swept one would simply be a no-op.
        sink.report(1, None, None);
        assert_eq!(store.get(&id).unwrap().progress.done, 1);
    }

    #[tokio::test]
    async fn checkpoint_reports_cancellation() {
        let cancel = CancellationToken::new();
        let (sink, _store, id, tick) = sink(Duration::ZERO);
        let ctx = JobContext::new(
            id,
            JobKey::Copy,
            Some(1),
            cancel.clone(),
            sink,
            BudgetSignal::full(),
            None,
            None,
        );
        assert!(ctx.checkpoint().await.is_ok());
        assert!(!ctx.is_cancelled());

        cancel.cancel();
        assert!(ctx.is_cancelled());
        assert!(matches!(ctx.checkpoint().await, Err(JobFailure::Cancelled)));
        assert!(tick.get() >= 2, "a checkpoint counts as liveness");
    }

    #[tokio::test]
    async fn run_blocking_returns_the_value_and_stops_when_cancelled() {
        let cancel = CancellationToken::new();
        let (sink, _store, id, _tick) = sink(Duration::ZERO);
        let ctx = JobContext::new(
            id,
            JobKey::Copy,
            Some(1),
            cancel.clone(),
            sink,
            BudgetSignal::full(),
            None,
            None,
        );
        assert_eq!(ctx.run_blocking(|| 7).await.unwrap(), 7);

        cancel.cancel();
        assert!(matches!(
            ctx.run_blocking(|| 7).await,
            Err(JobFailure::Cancelled)
        ));
    }

    #[tokio::test]
    async fn sleep_is_interrupted_by_cancellation_on_the_way_out() {
        let cancel = CancellationToken::new();
        let (sink, _store, id, _tick) = sink(Duration::ZERO);
        let ctx = JobContext::new(
            id,
            JobKey::Copy,
            Some(1),
            cancel.clone(),
            sink,
            BudgetSignal::full(),
            None,
            None,
        );
        cancel.cancel();
        assert!(matches!(
            ctx.sleep(Duration::from_secs(3600)).await,
            Err(JobFailure::Cancelled)
        ));
    }

    #[test]
    fn budget_defaults_to_full_and_clamps() {
        let b = BudgetSignal::full();
        assert_eq!(b.get(), 1.0);
        b.set_percent(40);
        assert_eq!(b.get(), 0.4);
        b.set_percent(500);
        assert_eq!(b.get(), 1.0);
    }
}
