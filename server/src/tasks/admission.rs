//! Measuring how busy the server is, and deciding whether a job may run.
//!
//! # Why this exists
//!
//! A background pass like garbage collection competes with the requests that
//! pay for the server. Running it while someone is syncing makes both slower;
//! running it never fills the disk. The workable compromise is to run it when
//! the server is quiet, and to stop making progress when it is not.
//!
//! # Why observation alone is not enough
//!
//! tokio cannot preempt a running task: nothing outside a job can suspend it,
//! lower its priority or take its CPU back. So a job only steps aside because it
//! checks in — see [`JobContext::checkpoint`](super::context::JobContext::checkpoint)
//! — and only shrinks because it reads
//! [`JobContext::budget`](super::context::JobContext::budget). This module
//! supplies the measurements; the contract that makes a job act on them is
//! declared per job in [`spec::QuietPolicy`](super::spec::QuietPolicy) and
//! enforced at registration.
//!
//! # Keeping a job from blocking itself
//!
//! The gauge separates *foreground* load (requests) from *background* load
//! (jobs). Admission looks only at the foreground: a GC pass raises block I/O
//! the moment it starts, so counting itself would make it defer forever.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU32, AtomicU64, Ordering};
use std::time::Duration;

/// What the server looked like at one moment.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct LoadSnapshot {
    /// Requests being served right now.
    pub inflight_requests: u64,
    /// Jobs running right now, which admission deliberately ignores.
    pub background_active: u64,
    /// Database connections in use, and how many exist.
    pub db_in_use: u64,
    pub db_max: u64,
    /// Worker threads busy, as a percentage.
    pub worker_busy_pct: u32,
    /// Tasks waiting to be polled across the runtime.
    pub queue_depth: u64,
    pub alive_tasks: u64,
    /// Unix time the sample was taken, so a stale sample is visible.
    pub sampled_at: i64,
}

impl LoadSnapshot {
    /// Database pool occupancy in `0.0..=1.0`.
    pub fn db_utilization(&self) -> f32 {
        if self.db_max == 0 {
            return 0.0;
        }
        self.db_in_use as f32 / self.db_max as f32
    }
}

/// The thresholds below which the server counts as quiet.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LoadThresholds {
    pub max_inflight_requests: u64,
    pub max_db_utilization: f32,
    pub max_worker_busy_pct: u32,
}

impl Default for LoadThresholds {
    fn default() -> Self {
        Self {
            max_inflight_requests: 8,
            max_db_utilization: 0.5,
            max_worker_busy_pct: 70,
        }
    }
}

impl LoadThresholds {
    /// Whether every foreground signal is below its threshold.
    ///
    /// `background_active` is not consulted: a job is not a reason for another
    /// job to wait, and counting it would let a long pass defer itself.
    pub fn is_calm(&self, snapshot: &LoadSnapshot) -> bool {
        snapshot.inflight_requests <= self.max_inflight_requests
            && snapshot.db_utilization() <= self.max_db_utilization
            && snapshot.worker_busy_pct <= self.max_worker_busy_pct
    }
}

/// What to do with a job right now.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Admission {
    /// Start it.
    Allow,
    /// Wait: the server is busy.
    Defer,
}

/// RAII counter for work that is in flight.
///
/// Dropped on every path, including a panic, so a counter cannot leak and make
/// the gauge read busy forever.
pub struct GaugeGuard {
    counter: Arc<AtomicU64>,
}

impl GaugeGuard {
    fn new(counter: Arc<AtomicU64>) -> Self {
        counter.fetch_add(1, Ordering::Relaxed);
        Self { counter }
    }
}

impl Drop for GaugeGuard {
    fn drop(&mut self) {
        self.counter.fetch_sub(1, Ordering::Relaxed);
    }
}

impl std::fmt::Debug for GaugeGuard {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("GaugeGuard")
    }
}

struct Inner {
    inflight_requests: Arc<AtomicU64>,
    background_active: Arc<AtomicU64>,
    db_in_use: AtomicU64,
    db_max: AtomicU64,
    worker_busy_pct: AtomicU32,
    queue_depth: AtomicU64,
    alive_tasks: AtomicU64,
    sampled_at: AtomicI64,
}

/// The process's one set of load counters.
#[derive(Clone)]
pub struct LoadGauge {
    inner: Arc<Inner>,
}

impl std::fmt::Debug for LoadGauge {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LoadGauge")
            .field("snapshot", &self.snapshot())
            .finish()
    }
}

impl Default for LoadGauge {
    fn default() -> Self {
        Self::new()
    }
}

impl LoadGauge {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Inner {
                inflight_requests: Arc::new(AtomicU64::new(0)),
                background_active: Arc::new(AtomicU64::new(0)),
                db_in_use: AtomicU64::new(0),
                db_max: AtomicU64::new(0),
                worker_busy_pct: AtomicU32::new(0),
                queue_depth: AtomicU64::new(0),
                alive_tasks: AtomicU64::new(0),
                sampled_at: AtomicI64::new(0),
            }),
        }
    }

    /// Count a request for as long as the guard lives.
    pub fn request_guard(&self) -> GaugeGuard {
        GaugeGuard::new(self.inner.inflight_requests.clone())
    }

    /// Count a job for as long as the guard lives.
    pub fn background_guard(&self) -> GaugeGuard {
        GaugeGuard::new(self.inner.background_active.clone())
    }

    pub fn set_db(&self, in_use: u64, max: u64) {
        self.inner.db_in_use.store(in_use, Ordering::Relaxed);
        self.inner.db_max.store(max, Ordering::Relaxed);
    }

    pub fn set_runtime(&self, worker_busy_pct: u32, queue_depth: u64, alive_tasks: u64) {
        self.inner
            .worker_busy_pct
            .store(worker_busy_pct, Ordering::Relaxed);
        self.inner.queue_depth.store(queue_depth, Ordering::Relaxed);
        self.inner.alive_tasks.store(alive_tasks, Ordering::Relaxed);
    }

    pub fn mark_sampled(&self, now: i64) {
        self.inner.sampled_at.store(now, Ordering::Relaxed);
    }

    pub fn snapshot(&self) -> LoadSnapshot {
        LoadSnapshot {
            inflight_requests: self.inner.inflight_requests.load(Ordering::Relaxed),
            background_active: self.inner.background_active.load(Ordering::Relaxed),
            db_in_use: self.inner.db_in_use.load(Ordering::Relaxed),
            db_max: self.inner.db_max.load(Ordering::Relaxed),
            worker_busy_pct: self.inner.worker_busy_pct.load(Ordering::Relaxed),
            queue_depth: self.inner.queue_depth.load(Ordering::Relaxed),
            alive_tasks: self.inner.alive_tasks.load(Ordering::Relaxed),
            sampled_at: self.inner.sampled_at.load(Ordering::Relaxed),
        }
    }

    /// Whether a sample is recent enough to act on.
    ///
    /// A sampler that stopped running must not look like a quiet server, or a
    /// stalled gauge would read as permanent permission to run everything.
    pub fn is_fresh(&self, max_age: Duration, now: i64) -> bool {
        let at = self.snapshot().sampled_at;
        at > 0 && now.saturating_sub(at) <= max_age.as_secs() as i64
    }
}

/// The gate a running job checks at its checkpoints.
///
/// Shares the gauge and the switch, so a job's idea of "busy" is the same as
/// the administrator's panel.
/// Shared by every job, so the interior mutability is in an `Arc`.
#[derive(Clone)]
pub struct YieldGate {
    inner: Arc<GateInner>,
}

struct GateInner {
    gauge: LoadGauge,
    /// Settable without `&mut`, because one gate is shared by every job.
    thresholds: std::sync::RwLock<LoadThresholds>,
    enabled: AtomicBool,
    /// How often a parked job re-checks, in milliseconds. Short enough to
    /// resume promptly, long enough not to spin.
    poll_ms: AtomicU64,
}

impl std::fmt::Debug for YieldGate {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("YieldGate")
            .field("enabled", &self.is_enabled())
            .field("snapshot", &self.inner.gauge.snapshot())
            .finish()
    }
}

impl YieldGate {
    pub fn new(gauge: LoadGauge, thresholds: LoadThresholds) -> Self {
        Self {
            inner: Arc::new(GateInner {
                gauge,
                thresholds: std::sync::RwLock::new(thresholds),
                enabled: AtomicBool::new(false),
                poll_ms: AtomicU64::new(200),
            }),
        }
    }

    /// Whether load-aware scheduling is switched on.
    pub fn is_enabled(&self) -> bool {
        self.inner.enabled.load(Ordering::Relaxed)
    }

    pub fn set_enabled(&self, enabled: bool) {
        self.inner.enabled.store(enabled, Ordering::Relaxed);
    }

    pub fn set_thresholds(&self, thresholds: LoadThresholds) {
        *self
            .inner
            .thresholds
            .write()
            .unwrap_or_else(|e| e.into_inner()) = thresholds;
    }

    /// The thresholds in force, for the administrator's view.
    pub fn thresholds(&self) -> LoadThresholds {
        *self
            .inner
            .thresholds
            .read()
            .unwrap_or_else(|e| e.into_inner())
    }

    /// Take a shorter poll interval, for tests.
    pub fn set_poll(&self, poll: Duration) {
        self.inner
            .poll_ms
            .store(poll.as_millis().max(1) as u64, Ordering::Relaxed);
    }

    /// Whether the server is calm enough for a parked job to continue.
    ///
    /// This — not the budget — is what a checkpoint tests: the budget is a
    /// scaling hint and can round to full speed while the server is still over
    /// a threshold of zero.
    pub fn is_calm(&self) -> bool {
        self.thresholds().is_calm(&self.inner.gauge.snapshot())
    }

    /// How much of its normal work a job should attempt now.
    ///
    /// Full speed when the server is calm or the feature is off; below that it
    /// scales with how far over the thresholds the worst signal is, down to a
    /// tenth — enough to keep making progress without competing.
    pub fn budget(&self) -> f32 {
        if !self.is_enabled() {
            return 1.0;
        }
        let thresholds = self.thresholds();
        let snapshot = self.inner.gauge.snapshot();
        if thresholds.is_calm(&snapshot) {
            return 1.0;
        }
        let ratios = [
            ratio(snapshot.inflight_requests, thresholds.max_inflight_requests),
            if thresholds.max_db_utilization > 0.0 {
                snapshot.db_utilization() / thresholds.max_db_utilization
            } else {
                0.0
            },
            if thresholds.max_worker_busy_pct > 0 {
                snapshot.worker_busy_pct as f32 / thresholds.max_worker_busy_pct as f32
            } else {
                0.0
            },
        ];
        let worst = ratios.into_iter().fold(0.0f32, f32::max);
        if worst <= 1.0 {
            1.0
        } else {
            (1.0 / worst).clamp(0.1, 1.0)
        }
    }

    /// Wait until the server is calm again, or until the run is cancelled.
    ///
    /// Returns `false` when the wait ended because of cancellation, so the
    /// caller can turn it into a terminal state rather than silently carrying
    /// on.
    pub async fn wait_until_calm(&self, cancel: &tokio_util::sync::CancellationToken) -> bool {
        loop {
            if !self.is_enabled() || self.is_calm() {
                return true;
            }
            let poll = Duration::from_millis(self.inner.poll_ms.load(Ordering::Relaxed).max(1));
            tokio::select! {
                _ = cancel.cancelled() => return false,
                _ = tokio::time::sleep(poll) => {}
            }
        }
    }
}

fn ratio(value: u64, threshold: u64) -> f32 {
    if value == 0 {
        return 0.0;
    }
    // A zero threshold means "any load at all is over it", so the ratio is
    // large rather than zero — otherwise the worst-case fold would read the
    // strictest signal as the most relaxed one.
    if threshold == 0 {
        return 1_000_000.0;
    }
    value as f32 / threshold as f32
}

/// Tracks how long the server has been quiet, so a job starts in a lull rather
/// than in the gap between two requests.
///
/// One tracker per job: the dwell a job wants is part of its own
/// [`QuietPolicy`](super::spec::QuietPolicy).
#[derive(Debug, Clone, Copy)]
pub struct QuietTracker {
    /// When the current calm window began, or `None` before the first
    /// observation. A busy sample pushes it forward.
    calm_since: Option<i64>,
}

impl Default for QuietTracker {
    fn default() -> Self {
        Self::new()
    }
}

impl QuietTracker {
    pub fn new() -> Self {
        Self { calm_since: None }
    }

    /// Whether the server has been calm for at least `min_idle_for_secs`.
    ///
    /// Records the observation either way, so a caller may poll this as often
    /// as it likes.
    pub fn admits(
        &mut self,
        snapshot: &LoadSnapshot,
        thresholds: &LoadThresholds,
        min_idle_for_secs: u64,
        now: i64,
    ) -> Admission {
        if !thresholds.is_calm(snapshot) {
            self.calm_since = Some(now);
            return Admission::Defer;
        }
        // The window opens on the first calm observation, so a job waits its
        // full dwell on a server that was never busy at all — which is what
        // keeps a start-up burst from running every deferrable job at once.
        let since = *self.calm_since.get_or_insert(now);
        if now.saturating_sub(since) >= min_idle_for_secs as i64 {
            Admission::Allow
        } else {
            Admission::Defer
        }
    }

    /// Forget the calm window, so a job that was forced or that just ran waits
    /// for a fresh one.
    pub fn reset(&mut self, now: i64) {
        self.calm_since = Some(now);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn calm() -> LoadSnapshot {
        LoadSnapshot {
            inflight_requests: 0,
            db_in_use: 0,
            db_max: 10,
            worker_busy_pct: 0,
            ..Default::default()
        }
    }

    #[test]
    fn a_guard_counts_up_and_down() {
        let gauge = LoadGauge::new();
        assert_eq!(gauge.snapshot().inflight_requests, 0);
        {
            let _a = gauge.request_guard();
            let _b = gauge.request_guard();
            assert_eq!(gauge.snapshot().inflight_requests, 2);
        }
        assert_eq!(gauge.snapshot().inflight_requests, 0);
    }

    /// A leaked counter would read as permanently busy and defer every job.
    #[test]
    fn a_dropped_guard_releases_the_counter() {
        let gauge = LoadGauge::new();
        let guard = gauge.request_guard();
        drop(guard);
        assert_eq!(gauge.snapshot().inflight_requests, 0);
    }

    #[test]
    fn blocking_work_is_counted_separately_from_requests() {
        let gauge = LoadGauge::new();
        let _job = gauge.background_guard();
        let snapshot = gauge.snapshot();
        assert_eq!(snapshot.background_active, 1);
        assert_eq!(snapshot.inflight_requests, 0);
    }

    /// The self-blocking trap: a job must not be able to defer itself.
    #[test]
    fn background_load_is_not_part_of_the_calm_check() {
        let thresholds = LoadThresholds::default();
        let mut snapshot = calm();
        snapshot.background_active = 10_000;
        assert!(
            thresholds.is_calm(&snapshot),
            "a job running is not a reason for another job to wait"
        );
    }

    #[test]
    fn each_foreground_signal_can_make_the_server_busy() {
        let thresholds = LoadThresholds {
            max_inflight_requests: 4,
            max_db_utilization: 0.5,
            max_worker_busy_pct: 70,
        };

        let mut busy = calm();
        busy.inflight_requests = 5;
        assert!(!thresholds.is_calm(&busy));

        let mut busy = calm();
        busy.db_in_use = 6;
        busy.db_max = 10;
        assert!(!thresholds.is_calm(&busy));

        let mut busy = calm();
        busy.worker_busy_pct = 71;
        assert!(!thresholds.is_calm(&busy));

        assert!(thresholds.is_calm(&calm()), "an idle server is calm");
    }

    #[test]
    fn db_utilization_handles_a_missing_pool() {
        let mut s = calm();
        s.db_max = 0;
        assert_eq!(s.db_utilization(), 0.0);
    }

    #[test]
    fn a_job_waits_for_the_dwell_even_on_a_quiet_server() {
        let thresholds = LoadThresholds::default();
        let mut tracker = QuietTracker::new();
        // The process starts; nothing has been busy yet, so the dwell starts now.
        assert_eq!(
            tracker.admits(&calm(), &thresholds, 30, 1_000),
            Admission::Defer
        );
        assert_eq!(
            tracker.admits(&calm(), &thresholds, 30, 1_020),
            Admission::Defer
        );
        assert_eq!(
            tracker.admits(&calm(), &thresholds, 30, 1_030),
            Admission::Allow
        );
    }

    #[test]
    fn a_busy_moment_restarts_the_dwell() {
        let thresholds = LoadThresholds::default();
        let mut tracker = QuietTracker::new();
        assert_eq!(
            tracker.admits(&calm(), &thresholds, 30, 1_000),
            Admission::Defer
        );
        assert_eq!(
            tracker.admits(&calm(), &thresholds, 30, 1_100),
            Admission::Allow
        );

        let mut busy = calm();
        busy.inflight_requests = 99;
        assert_eq!(
            tracker.admits(&busy, &thresholds, 30, 1_200),
            Admission::Defer
        );
        // The dwell now dates from the busy moment, not from start-up.
        assert_eq!(
            tracker.admits(&calm(), &thresholds, 30, 1_220),
            Admission::Defer
        );
        assert_eq!(
            tracker.admits(&calm(), &thresholds, 30, 1_230),
            Admission::Allow
        );
    }

    #[test]
    fn a_zero_dwell_admits_immediately_once_calm() {
        let thresholds = LoadThresholds::default();
        let mut tracker = QuietTracker::new();
        assert_eq!(
            tracker.admits(&calm(), &thresholds, 0, 1_000),
            Admission::Allow
        );
    }

    #[test]
    fn reset_pushes_the_window_back() {
        let thresholds = LoadThresholds::default();
        let mut tracker = QuietTracker::new();
        tracker.admits(&calm(), &thresholds, 0, 0);
        tracker.reset(1_000);
        assert_eq!(
            tracker.admits(&calm(), &thresholds, 30, 1_010),
            Admission::Defer
        );
        assert_eq!(
            tracker.admits(&calm(), &thresholds, 30, 1_030),
            Admission::Allow
        );
    }

    /// A stalled sampler must not read as a quiet server.
    #[test]
    fn a_stale_sample_is_not_fresh() {
        let gauge = LoadGauge::new();
        assert!(
            !gauge.is_fresh(Duration::from_secs(15), 1_000),
            "never sampled"
        );
        gauge.mark_sampled(1_000);
        assert!(gauge.is_fresh(Duration::from_secs(15), 1_010));
        assert!(!gauge.is_fresh(Duration::from_secs(15), 1_100));
    }

    fn gate_with(gauge: LoadGauge, thresholds: LoadThresholds, enabled: bool) -> YieldGate {
        let gate = YieldGate::new(gauge, thresholds);
        gate.set_enabled(enabled);
        gate.set_poll(Duration::from_millis(1));
        gate
    }

    fn gate(gauge: LoadGauge) -> YieldGate {
        gate_with(gauge, LoadThresholds::default(), false)
    }

    #[test]
    fn a_disabled_gate_never_yields_and_offers_full_budget() {
        let gauge = LoadGauge::new();
        let _busy: Vec<_> = (0..100).map(|_| gauge.request_guard()).collect();
        let gate = gate(gauge.clone());
        assert!(!gate.is_enabled());
        assert_eq!(gate.budget(), 1.0, "a switched-off gate is never busy");
    }

    #[test]
    fn budget_scales_with_how_far_over_the_server_is() {
        let gauge = LoadGauge::new();
        let gate = gate_with(
            gauge.clone(),
            LoadThresholds {
                max_inflight_requests: 4,
                max_db_utilization: 0.5,
                max_worker_busy_pct: 70,
            },
            true,
        );

        assert_eq!(gate.budget(), 1.0, "an idle server allows full speed");

        // Exactly at the threshold is still calm.
        let _at = (0..4).map(|_| gauge.request_guard()).collect::<Vec<_>>();
        assert_eq!(gate.budget(), 1.0);

        // Twice the threshold halves the budget.
        let _over = (0..4).map(|_| gauge.request_guard()).collect::<Vec<_>>();
        assert!((gate.budget() - 0.5).abs() < 0.01, "got {}", gate.budget());

        // A very busy server still makes some progress.
        let _flood: Vec<_> = (0..96).map(|_| gauge.request_guard()).collect();
        assert!(gate.budget() >= 0.1);
        assert!(gate.budget() < 0.2);
    }

    #[tokio::test]
    async fn a_parked_job_resumes_when_the_server_goes_quiet() {
        let gauge = LoadGauge::new();
        let gate = gate_with(gauge.clone(), LoadThresholds::default(), true);
        let busy = (0..20).map(|_| gauge.request_guard()).collect::<Vec<_>>();
        let cancel = tokio_util::sync::CancellationToken::new();

        // While busy, the wait does not return.
        assert!(
            tokio::time::timeout(Duration::from_millis(20), gate.wait_until_calm(&cancel))
                .await
                .is_err(),
            "a busy server must keep the job parked"
        );

        drop(busy);
        assert!(
            tokio::time::timeout(Duration::from_secs(1), gate.wait_until_calm(&cancel))
                .await
                .expect("the job must resume once the server is quiet"),
            "resuming is not a cancellation"
        );
    }

    #[tokio::test]
    async fn a_parked_job_returns_immediately_when_cancelled() {
        let gauge = LoadGauge::new();
        let gate = gate_with(gauge.clone(), LoadThresholds::default(), true);
        let _busy = (0..20).map(|_| gauge.request_guard()).collect::<Vec<_>>();
        let cancel = tokio_util::sync::CancellationToken::new();
        cancel.cancel();
        assert!(
            !gate.wait_until_calm(&cancel).await,
            "the wait reports cancellation rather than calm"
        );
    }

    /// The self-blocking trap, at the level that matters: a job's own activity
    /// must not keep it parked.
    #[tokio::test]
    async fn a_jobs_own_activity_does_not_park_it() {
        let gauge = LoadGauge::new();
        let gate = gate_with(gauge.clone(), LoadThresholds::default(), true);
        let _background = (0..50)
            .map(|_| gauge.background_guard())
            .collect::<Vec<_>>();
        let cancel = tokio_util::sync::CancellationToken::new();
        assert!(
            tokio::time::timeout(Duration::from_millis(50), gate.wait_until_calm(&cancel))
                .await
                .expect("its own load must not park a job"),
        );
        assert_eq!(gate.budget(), 1.0);
    }

    #[test]
    fn the_snapshot_reports_what_was_set() {
        let gauge = LoadGauge::new();
        gauge.set_db(3, 12);
        gauge.set_runtime(40, 5, 9);
        gauge.mark_sampled(77);
        let s = gauge.snapshot();
        assert_eq!(s.db_in_use, 3);
        assert_eq!(s.db_max, 12);
        assert_eq!(s.worker_busy_pct, 40);
        assert_eq!(s.queue_depth, 5);
        assert_eq!(s.alive_tasks, 9);
        assert_eq!(s.sampled_at, 77);
    }
}
