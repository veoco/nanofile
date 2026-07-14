//! Unified lifecycle manager for periodic and continuous background tasks.
//!
//! All background tasks in the application are registered through [`Scheduler`],
//! which provides consistent logging, result collection, and runtime metrics.
//!
//! # Task kinds
//!
//! * **Periodic** — runs on a fixed interval ([`Scheduler::spawn_periodic`]).
//!   The scheduler manages the interval loop and collects [`TaskOutput`] after
//!   every execution.
//! * **Continuous** — runs until cancelled ([`Scheduler::spawn_continuous`]).
//!   Used for event-driven tasks that don't have a natural tick interval.

use std::future::Future;
use std::sync::Arc;
use std::sync::Mutex;
use std::time::Duration;
use tokio::sync::RwLock;
use tokio_util::sync::CancellationToken;

/// The outcome of a single task execution.
#[derive(Debug, Clone)]
pub struct TaskOutput {
    /// Whether the execution succeeded.
    pub success: bool,
    /// Human-readable message (e.g. "Cleaned up 5 expired share links").
    pub message: String,
    /// Number of items processed, if applicable.
    pub processed_count: Option<u64>,
}

impl TaskOutput {
    /// Create a success result.
    pub fn success(message: impl Into<String>, processed_count: Option<u64>) -> Self {
        Self {
            success: true,
            message: message.into(),
            processed_count,
        }
    }

    /// Create an error result.
    pub fn error(message: impl Into<String>) -> Self {
        Self {
            success: false,
            message: message.into(),
            processed_count: None,
        }
    }
}

/// Runtime metrics accumulated across all executions of a task.
#[derive(Debug, Clone, Default)]
pub struct TaskMetrics {
    /// Total number of executions.
    pub run_count: u64,
    /// Number of successful executions.
    pub success_count: u64,
    /// Number of failed executions.
    pub error_count: u64,
    /// Unix timestamp of the most recent execution, or `None` if never run.
    pub last_run_at: Option<i64>,
    /// Duration of the most recent execution in milliseconds.
    pub last_duration_ms: u64,
    /// Message from the most recent execution.
    pub last_message: String,
    /// Cumulative number of items processed across all runs.
    pub total_processed: u64,
}

/// Whether a task runs periodically or continuously.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskKind {
    /// Task runs on a fixed interval.
    Periodic { interval_secs: u64 },
    /// Task runs until cancelled (event-driven).
    Continuous,
}

/// A handle to inspect a running task's metadata and runtime metrics.
#[derive(Clone)]
pub struct TaskHandle {
    /// Human-readable task name (e.g. `"share link cleanup"`).
    pub name: &'static str,
    /// Whether the task is periodic or continuous.
    pub kind: TaskKind,
    /// Shared runtime metrics.
    pub(super) metrics: Arc<RwLock<TaskMetrics>>,
}

impl TaskHandle {
    /// Snapshot the current runtime metrics for this task.
    pub async fn metrics(&self) -> TaskMetrics {
        self.metrics.read().await.clone()
    }
}

/// Central registry for all background tasks.
///
/// ```ignore
/// let scheduler = Scheduler::new(shutdown_token.child_token());
///
/// // Periodic task: scheduler manages the interval loop.
/// scheduler.spawn_periodic("housekeeping", 3600, || async {
///     do_work().await;
///     TaskOutput::success("done", Some(count))
/// });
///
/// // Continuous task: runs until cancellation.
/// scheduler.spawn_continuous("event listener", |token| async move {
///     listen_loop(token).await;
/// });
/// ```
pub struct Scheduler {
    shutdown_token: CancellationToken,
    handles: Mutex<Vec<TaskHandle>>,
}

impl Scheduler {
    /// Create a new scheduler.
    ///
    /// When `shutdown_token` is cancelled, all registered tasks are signalled
    /// to stop.
    pub fn new(shutdown_token: CancellationToken) -> Self {
        Self {
            shutdown_token,
            handles: Mutex::new(Vec::new()),
        }
    }

    /// Register and spawn a **periodic** task.
    ///
    /// The scheduler manages the full interval loop, collects [`TaskOutput`]
    /// after every execution, and records [`TaskMetrics`].
    ///
    /// The first execution waits one full interval before running (the initial
    /// tick is consumed during setup).
    pub fn spawn_periodic<F, Fut>(
        &self,
        name: &'static str,
        interval_secs: u64,
        work: F,
    ) -> TaskHandle
    where
        F: Fn() -> Fut + Send + 'static,
        Fut: Future<Output = TaskOutput> + Send,
    {
        let metrics = Arc::new(RwLock::new(TaskMetrics::default()));
        let m = metrics.clone();
        let child = self.shutdown_token.child_token();

        tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(interval_secs));
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            // Consume the first tick so the first execution waits one full interval.
            interval.tick().await;

            tracing::info!(name, interval_secs, "Scheduled task started");

            loop {
                tokio::select! {
                    _ = child.cancelled() => {
                        tracing::info!(name, "Scheduled task stopped");
                        break;
                    }
                    _ = interval.tick() => {
                        let start = std::time::Instant::now();
                        let output = work().await;
                        let elapsed = start.elapsed();

                        let mut stats = m.write().await;
                        stats.run_count += 1;
                        stats.last_run_at = Some(chrono::Utc::now().timestamp());
                        stats.last_duration_ms = elapsed.as_millis() as u64;
                        stats.last_message = output.message.clone();

                        if output.success {
                            stats.success_count += 1;
                            if let Some(count) = output.processed_count {
                                stats.total_processed += count;
                            }
                            tracing::debug!(name, message = %output.message, "Periodic task completed");
                        } else {
                            stats.error_count += 1;
                            tracing::warn!(name, message = %output.message, "Periodic task failed");
                        }
                    }
                }
            }
        });

        let handle = TaskHandle {
            name,
            kind: TaskKind::Periodic { interval_secs },
            metrics,
        };
        self.handles.lock().unwrap().push(handle.clone());
        handle
    }

    /// Register and spawn a **continuous** task.
    ///
    /// The task receives a [`CancellationToken`] and should run until it is
    /// cancelled. The scheduler only provides lifecycle logging — the task
    /// owns its own loop.
    pub fn spawn_continuous<F, Fut>(&self, name: &'static str, task: F) -> TaskHandle
    where
        F: FnOnce(CancellationToken) -> Fut + Send + 'static,
        Fut: Future<Output = ()> + Send,
    {
        let child = self.shutdown_token.child_token();
        let metrics = Arc::new(RwLock::new(TaskMetrics::default()));

        tracing::info!(name, "Continuous task started");
        tokio::spawn(async move {
            task(child).await;
            tracing::info!(name, "Continuous task stopped");
        });

        let handle = TaskHandle {
            name,
            kind: TaskKind::Continuous,
            metrics,
        };
        self.handles.lock().unwrap().push(handle.clone());
        handle
    }

    /// Return a snapshot of all registered task handles.
    pub fn handles(&self) -> Vec<TaskHandle> {
        self.handles.lock().unwrap().clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    #[tokio::test]
    async fn test_periodic_spawns_and_cancels() {
        let token = CancellationToken::new();
        let sched = Scheduler::new(token.child_token());
        let counter = Arc::new(AtomicU64::new(0));

        sched.spawn_periodic("test-periodic", 1, {
            let c = counter.clone();
            move || {
                let c = c.clone();
                async move {
                    c.fetch_add(1, Ordering::SeqCst);
                    TaskOutput::success("ok", None)
                }
            }
        });

        tokio::time::sleep(Duration::from_millis(1500)).await;
        token.cancel();
        let count = counter.load(Ordering::SeqCst);
        assert!(
            count >= 1,
            "task should have run at least once, got {count}"
        );
    }

    #[tokio::test]
    async fn test_periodic_metrics() {
        let token = CancellationToken::new();
        let sched = Scheduler::new(token.child_token());

        let handle = sched.spawn_periodic("metrics-test", 1, move || async {
            TaskOutput::success("all good", Some(42))
        });

        tokio::time::sleep(Duration::from_millis(1200)).await;
        token.cancel();

        let m = handle.metrics().await;
        assert!(m.run_count >= 1, "run_count={}", m.run_count);
        assert!(m.last_run_at.is_some());
        assert_eq!(m.last_message, "all good");
        assert_eq!(m.total_processed, 42 * m.run_count);
    }

    #[tokio::test]
    async fn test_periodic_error_tracking() {
        let token = CancellationToken::new();
        let sched = Scheduler::new(token.child_token());

        let handle = sched.spawn_periodic("error-test", 1, move || async {
            TaskOutput::error("something went wrong")
        });

        tokio::time::sleep(Duration::from_millis(1200)).await;
        token.cancel();

        let m = handle.metrics().await;
        assert!(m.run_count >= 1);
        assert_eq!(m.error_count, m.run_count);
        assert_eq!(m.success_count, 0);
        assert!(m.last_message.contains("went wrong"));
    }

    #[tokio::test]
    async fn test_continuous_stops_on_cancel() {
        let token = CancellationToken::new();
        let sched = Scheduler::new(token.child_token());
        let started = Arc::new(AtomicU64::new(0));

        sched.spawn_continuous("test-continuous", {
            let s = started.clone();
            move |ct| {
                let s = s.clone();
                async move {
                    s.fetch_add(1, Ordering::SeqCst);
                    ct.cancelled().await;
                }
            }
        });

        tokio::time::sleep(Duration::from_millis(100)).await;
        assert_eq!(started.load(Ordering::SeqCst), 1);
        token.cancel();
    }

    #[tokio::test]
    async fn test_multiple_tasks_independent() {
        let token = CancellationToken::new();
        let sched = Scheduler::new(token.child_token());
        let c1 = Arc::new(AtomicU64::new(0));
        let c2 = Arc::new(AtomicU64::new(0));

        sched.spawn_periodic("a", 1, {
            let c = c1.clone();
            move || {
                let c = c.clone();
                async move {
                    c.fetch_add(1, Ordering::SeqCst);
                    TaskOutput::success("a", None)
                }
            }
        });
        sched.spawn_periodic("b", 1, {
            let c = c2.clone();
            move || {
                let c = c.clone();
                async move {
                    c.fetch_add(1, Ordering::SeqCst);
                    TaskOutput::success("b", None)
                }
            }
        });

        tokio::time::sleep(Duration::from_millis(1500)).await;
        token.cancel();
        assert!(c1.load(Ordering::SeqCst) >= 1);
        assert!(c2.load(Ordering::SeqCst) >= 1);
    }

    #[tokio::test]
    async fn test_handles_returns_all_tasks() {
        let token = CancellationToken::new();
        let sched = Scheduler::new(token.child_token());

        sched.spawn_periodic("p1", 60, || async { TaskOutput::success("ok", None) });
        sched.spawn_continuous("c1", |ct| async move { ct.cancelled().await });

        let handles = sched.handles();
        assert_eq!(handles.len(), 2);
        assert!(handles.iter().any(|h| h.name == "p1"));
        assert!(handles.iter().any(|h| h.name == "c1"));
    }
}
