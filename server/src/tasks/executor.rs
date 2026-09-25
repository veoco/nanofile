//! Driving one run to a terminal state.
//!
//! This is the only place the run state machine is written. A handler submits
//! and forgets: it cannot leave a run stuck in `Queued`, because it has no way
//! to transition one, and it cannot leak a concurrency slot, because the permit
//! lives in the task this module owns and is released whatever the body does —
//! including panicking.

use std::future::Future;
use std::panic::AssertUnwindSafe;
use std::time::Duration;

use futures_util::FutureExt;
use tokio_util::sync::CancellationToken;

use super::context::{BudgetSignal, JobContext, LivenessTick, ProgressSink};
use super::registry::RegisteredJob;
use super::run::{JobFailure, JobRun, JobState, Outcome, Params};
use super::spec::TimeoutPolicy;
use super::store::RunStore;

/// How often progress writes are coalesced for one run.
const PROGRESS_THROTTLE: Duration = Duration::from_secs(1);

/// What the executor produced, for logging and (later) the audit record.
#[derive(Debug)]
pub struct RunReport {
    pub state: JobState,
    pub outcome: Option<Outcome>,
    pub attempts: u32,
    /// When the first attempt started, as the store recorded it.
    pub started_at: Option<i64>,
    /// When the run reached its terminal state, as the store recorded it.
    pub finished_at: Option<i64>,
}

/// Drive `run`'s body to completion, writing every state transition.
///
/// `cancel` is the run's own token; the caller keeps a handle so it can ask the
/// run to stop.
pub async fn execute(
    job: &RegisteredJob,
    run: &JobRun,
    params: Params,
    store: &RunStore,
    cancel: CancellationToken,
    gate: Option<super::admission::YieldGate>,
) -> RunReport {
    let spec = &job.spec;
    let tick = LivenessTick::default();
    let sink = ProgressSink::new(
        store.clone(),
        run.id.clone(),
        PROGRESS_THROTTLE,
        tick.clone(),
    );
    let ctx = JobContext::new(
        run.id.clone(),
        spec.key,
        run.owner,
        cancel,
        sink,
        BudgetSignal::full(),
        spec.chunkable,
        gate,
    );

    let mut attempt = 1u32;
    let attempts_allowed = spec.retry.attempts().max(1);

    loop {
        let started_at = mark_running(store, run, attempt);
        match attempt_once(job, &ctx, params.clone(), &tick).await {
            Ok(outcome) => {
                let finished_at =
                    mark_terminal(store, run, JobState::Succeeded, Some(outcome.clone()));
                return RunReport {
                    state: JobState::Succeeded,
                    outcome: Some(outcome),
                    attempts: attempt,
                    started_at: Some(started_at),
                    finished_at: Some(finished_at),
                };
            }
            // A cancel or a timeout is terminal: retrying would either ignore
            // the caller or repeat whatever caused the stall.
            Err(JobFailure::Cancelled) => {
                let finished_at = mark_terminal(store, run, JobState::Cancelled, None);
                return RunReport {
                    state: JobState::Cancelled,
                    outcome: None,
                    attempts: attempt,
                    started_at: Some(started_at),
                    finished_at: Some(finished_at),
                };
            }
            Err(JobFailure::TimedOut) => {
                let finished_at = mark_terminal(store, run, JobState::TimedOut, None);
                return RunReport {
                    state: JobState::TimedOut,
                    outcome: None,
                    attempts: attempt,
                    started_at: Some(started_at),
                    finished_at: Some(finished_at),
                };
            }
            Err(JobFailure::App(e)) => {
                if attempt >= attempts_allowed {
                    let state = JobState::Failed(e.to_string());
                    let finished_at = mark_terminal(store, run, state.clone(), None);
                    return RunReport {
                        state,
                        outcome: None,
                        attempts: attempt,
                        started_at: Some(started_at),
                        finished_at: Some(finished_at),
                    };
                }
                tracing::warn!(
                    job = spec.name,
                    run = %run.id,
                    attempt,
                    error = %e,
                    "job failed; retrying"
                );
                let delay = Duration::from_secs(spec.retry.delay_secs(attempt));
                if !delay.is_zero() && ctx.sleep(delay).await.is_err() {
                    let finished_at = mark_terminal(store, run, JobState::Cancelled, None);
                    return RunReport {
                        state: JobState::Cancelled,
                        outcome: None,
                        attempts: attempt,
                        started_at: Some(started_at),
                        finished_at: Some(finished_at),
                    };
                }
                attempt += 1;
            }
        }
    }
}

/// One attempt: the body, with a panic guard around it and the timeout policy
/// applied on top.
async fn attempt_once(
    job: &RegisteredJob,
    ctx: &JobContext,
    params: Params,
    tick: &LivenessTick,
) -> Result<Outcome, JobFailure> {
    let body = (job.run)(ctx.clone(), params);
    // Panic isolation: a body that panics must still land in a terminal state
    // and release its slot, which is what the old copy/move manager could not
    // do (a panicking task held its active slot forever).
    let guarded = AssertUnwindSafe(body).catch_unwind();

    let caught = match job.spec.timeout {
        TimeoutPolicy {
            no_progress_for_secs: None,
            max_total_secs: None,
        } => guarded.await,
        policy => watch(guarded, policy, tick).await,
    };
    flatten(caught)
}

/// What a guarded body resolves to: the body's own result, or the panic payload
/// it unwound with.
type Caught = Result<Result<Outcome, JobFailure>, Box<dyn std::any::Any + Send>>;

/// Run `body` under the timeout policy.
///
/// Two different questions are asked. `no_progress_for` is the one that matters
/// for a long job: it fails a run that has stopped getting anywhere, without
/// guessing how long the whole thing should take. `max_total` is the backstop.
async fn watch<F>(body: F, policy: TimeoutPolicy, tick: &LivenessTick) -> Caught
where
    F: Future<Output = Caught>,
{
    let started = tokio::time::Instant::now();
    let mut seen = tick.get();
    tokio::pin!(body);

    loop {
        let progress_window = policy.no_progress_for_secs.map(Duration::from_secs);
        let total_left = policy.max_total_secs.map(|max| {
            Duration::from_secs(max)
                .checked_sub(started.elapsed())
                .unwrap_or(Duration::ZERO)
        });

        tokio::select! {
            biased;
            result = &mut body => return result,
            _ = sleep_opt(progress_window), if progress_window.is_some() => {
                let now = tick.get();
                if now == seen {
                    return Ok(Err(JobFailure::TimedOut));
                }
                seen = now;
            }
            _ = sleep_opt(total_left), if total_left.is_some() => {
                return Ok(Err(JobFailure::TimedOut));
            }
        }
    }
}

/// A sleep that is never constructed when the deadline is absent, so an unset
/// timeout cannot fire.
async fn sleep_opt(duration: Option<Duration>) {
    match duration {
        Some(d) => tokio::time::sleep(d).await,
        // The select arms that call this are gated on `is_some`, so this is
        // unreachable; pending forever keeps the arm inert if that ever slips.
        None => std::future::pending::<()>().await,
    }
}

/// Turn a caught panic into a failure, keeping the panic message when it is
/// readable.
fn flatten(result: Caught) -> Result<Outcome, JobFailure> {
    match result {
        Ok(inner) => inner,
        Err(payload) => {
            let detail = payload
                .downcast_ref::<&str>()
                .map(|s| (*s).to_string())
                .or_else(|| payload.downcast_ref::<String>().cloned())
                .unwrap_or_else(|| "unknown panic".to_string());
            Err(JobFailure::App(base::error::AppError::internal(format!(
                "job panicked: {detail}"
            ))))
        }
    }
}

/// Mark the run as executing. Returns when it started: the first attempt's
/// timestamp, which is the one the run keeps.
fn mark_running(store: &RunStore, run: &JobRun, attempt: u32) -> i64 {
    let now = chrono::Utc::now().timestamp();
    let id = run.id.clone();
    let mut started_at = now;
    store.update(&id, |r| {
        r.state = JobState::Running;
        r.started_at.get_or_insert(now);
        started_at = r.started_at.unwrap_or(now);
        r.attempt = attempt;
    });
    started_at
}

/// Mark the run terminal. Returns when it finished.
fn mark_terminal(store: &RunStore, run: &JobRun, state: JobState, outcome: Option<Outcome>) -> i64 {
    let now = chrono::Utc::now().timestamp();
    let id = run.id.clone();
    store.update(&id, |r| {
        if let Some(outcome) = &outcome {
            r.progress.message = outcome.message.clone();
            if let Some(processed) = outcome.processed {
                r.progress.done = processed;
            }
        }
        r.state = state;
        r.finished_at = Some(now);
        // The submit-time input can be large (a batch of file names) and has
        // served its purpose; a wire projection reads the summary instead.
        r.drop_params();
    });
    now
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tasks::spec::{
        Dedup, Durability, JobKey, JobSpec, Priority, Resource, Retention, RetryPolicy, Trigger,
        Visibility,
    };
    use crate::tasks::store::RunLimits;
    use std::pin::Pin;
    use std::sync::Arc;

    fn job_with(
        retry: RetryPolicy,
        timeout: TimeoutPolicy,
        run: impl Fn(
            JobContext,
            Params,
        ) -> Pin<Box<dyn Future<Output = Result<Outcome, JobFailure>> + Send>>
        + Send
        + Sync
        + 'static,
    ) -> RegisteredJob {
        RegisteredJob::new(
            JobSpec {
                key: JobKey::Copy,
                name: "test-job",
                trigger: Trigger::OnDemand,
                priority: Priority::Normal,
                resource: Resource::Cpu,
                max_concurrent: 1,
                queue_depth: 0,
                timeout,
                retry,
                dedup: Dedup::None,
                retention: Retention::default(),
                visibility: Visibility::Owner,
                durability: Durability::Memory,
                resumable: true,
                cancellable: true,
                chunkable: None,
                force_safe: false,
                quiet: None,
                idempotent: true,
            },
            Arc::new(run),
        )
    }

    fn queued() -> JobRun {
        JobRun::queued(
            JobKey::Copy,
            Visibility::Owner,
            Some(1),
            serde_json::json!({"src_dirents": ["a"]}),
            "Copy 1 item",
            Some(1),
            0,
        )
    }

    fn store() -> RunStore {
        RunStore::new(RunLimits::default())
    }

    #[tokio::test]
    async fn a_successful_body_lands_in_succeeded() {
        let store = store();
        let run = queued();
        store.insert(run.clone()).unwrap();
        let job = job_with(RetryPolicy::Never, TimeoutPolicy::default(), |_ctx, _p| {
            Box::pin(async { Ok(Outcome::success("done", Some(3))) })
        });

        let report = execute(
            &job,
            &run,
            serde_json::json!({}),
            &store,
            CancellationToken::new(),
            None,
        )
        .await;
        assert_eq!(report.state, JobState::Succeeded);
        assert_eq!(report.attempts, 1);

        let stored = store.get(&run.id).unwrap();
        assert_eq!(stored.state, JobState::Succeeded);
        assert!(stored.finished_at.is_some());
        assert!(stored.started_at.is_some());
        assert_eq!(stored.progress.done, 3);
        assert_eq!(stored.progress.message, "done");
        assert!(stored.params.is_none(), "params are released at the end");
    }

    #[tokio::test]
    async fn a_failing_body_lands_in_failed_with_the_message() {
        let store = store();
        let run = queued();
        store.insert(run.clone()).unwrap();
        let job = job_with(RetryPolicy::Never, TimeoutPolicy::default(), |_ctx, _p| {
            Box::pin(async {
                Err(JobFailure::App(base::error::AppError::BadRequest(
                    "nope".into(),
                )))
            })
        });

        let report = execute(
            &job,
            &run,
            serde_json::json!({}),
            &store,
            CancellationToken::new(),
            None,
        )
        .await;
        match report.state {
            JobState::Failed(message) => assert!(message.contains("nope")),
            other => panic!("expected Failed, got {other:?}"),
        }
        assert!(matches!(
            store.get(&run.id).unwrap().state,
            JobState::Failed(_)
        ));
    }

    /// The documented defect in the old copy/move manager: a panicking task held
    /// its active slot forever. Here the run lands in a terminal state, so the
    /// slot is released by the caller's guard.
    #[tokio::test]
    async fn a_panicking_body_lands_in_failed() {
        let store = store();
        let run = queued();
        store.insert(run.clone()).unwrap();
        let job = job_with(RetryPolicy::Never, TimeoutPolicy::default(), |_ctx, _p| {
            Box::pin(async { panic!("boom") })
        });

        let report = execute(
            &job,
            &run,
            serde_json::json!({}),
            &store,
            CancellationToken::new(),
            None,
        )
        .await;
        match &report.state {
            JobState::Failed(message) => {
                assert!(message.contains("panicked"), "got {message}");
                assert!(message.contains("boom"), "the panic message is kept");
            }
            other => panic!("expected Failed, got {other:?}"),
        }
        let stored = store.get(&run.id).unwrap();
        assert!(stored.state.is_terminal());
        assert!(stored.finished_at.is_some());
    }

    #[tokio::test]
    async fn a_cancelled_body_lands_in_cancelled_and_is_not_retried() {
        let store = store();
        let run = queued();
        store.insert(run.clone()).unwrap();
        let job = job_with(
            RetryPolicy::Fixed {
                attempts: 5,
                delay_secs: 0,
            },
            TimeoutPolicy::default(),
            |_ctx, _p| Box::pin(async { Err(JobFailure::Cancelled) }),
        );

        let report = execute(
            &job,
            &run,
            serde_json::json!({}),
            &store,
            CancellationToken::new(),
            None,
        )
        .await;
        assert_eq!(report.state, JobState::Cancelled);
        assert_eq!(report.attempts, 1, "a cancel is never retried");
    }

    #[tokio::test]
    async fn a_timed_out_body_lands_in_timed_out() {
        let store = store();
        let run = queued();
        store.insert(run.clone()).unwrap();
        let job = job_with(RetryPolicy::Never, TimeoutPolicy::default(), |_ctx, _p| {
            Box::pin(async { Err(JobFailure::TimedOut) })
        });

        let report = execute(
            &job,
            &run,
            serde_json::json!({}),
            &store,
            CancellationToken::new(),
            None,
        )
        .await;
        assert_eq!(report.state, JobState::TimedOut);
    }

    #[tokio::test]
    async fn retries_until_the_attempt_budget_is_spent() {
        let store = store();
        let run = queued();
        store.insert(run.clone()).unwrap();
        let attempts = Arc::new(std::sync::atomic::AtomicU32::new(0));
        let counter = attempts.clone();
        let job = job_with(
            RetryPolicy::Fixed {
                attempts: 3,
                delay_secs: 0,
            },
            TimeoutPolicy::default(),
            move |_ctx, _p| {
                let counter = counter.clone();
                Box::pin(async move {
                    counter.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    Err(JobFailure::App(base::error::AppError::Internal(
                        "flaky".into(),
                    )))
                })
            },
        );

        let report = execute(
            &job,
            &run,
            serde_json::json!({}),
            &store,
            CancellationToken::new(),
            None,
        )
        .await;
        assert_eq!(attempts.load(std::sync::atomic::Ordering::SeqCst), 3);
        assert_eq!(report.attempts, 3);
        assert!(matches!(report.state, JobState::Failed(_)));
    }

    #[tokio::test]
    async fn a_retry_that_succeeds_reports_one_attempt_used() {
        let store = store();
        let run = queued();
        store.insert(run.clone()).unwrap();
        let attempts = Arc::new(std::sync::atomic::AtomicU32::new(0));
        let counter = attempts.clone();
        let job = job_with(
            RetryPolicy::Fixed {
                attempts: 3,
                delay_secs: 0,
            },
            TimeoutPolicy::default(),
            move |_ctx, _p| {
                let counter = counter.clone();
                Box::pin(async move {
                    let n = counter.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    if n == 0 {
                        Err(JobFailure::App(base::error::AppError::Internal(
                            "first try".into(),
                        )))
                    } else {
                        Ok(Outcome::success("second try", None))
                    }
                })
            },
        );

        let report = execute(
            &job,
            &run,
            serde_json::json!({}),
            &store,
            CancellationToken::new(),
            None,
        )
        .await;
        assert_eq!(report.state, JobState::Succeeded);
        assert_eq!(report.attempts, 2);
    }

    /// The watchdog fails a run that stops checking in, which is the failure
    /// mode a wall-clock timeout cannot distinguish from legitimately slow work.
    #[tokio::test(start_paused = true)]
    async fn a_stalled_body_is_timed_out_by_the_progress_watchdog() {
        let store = store();
        let run = queued();
        store.insert(run.clone()).unwrap();
        let job = job_with(
            RetryPolicy::Never,
            TimeoutPolicy {
                no_progress_for_secs: Some(30),
                max_total_secs: None,
            },
            |_ctx, _p| {
                Box::pin(async {
                    tokio::time::sleep(Duration::from_secs(3600)).await;
                    Ok(Outcome::ok())
                })
            },
        );

        let report = execute(
            &job,
            &run,
            serde_json::json!({}),
            &store,
            CancellationToken::new(),
            None,
        )
        .await;
        assert_eq!(report.state, JobState::TimedOut);
    }

    /// A body that keeps checking in is not killed, however long it runs: that
    /// is the whole point of watching progress rather than wall-clock time.
    #[tokio::test(start_paused = true)]
    async fn a_body_that_keeps_checking_in_is_not_timed_out() {
        let store = store();
        let run = queued();
        store.insert(run.clone()).unwrap();
        let job = job_with(
            RetryPolicy::Never,
            TimeoutPolicy {
                no_progress_for_secs: Some(30),
                max_total_secs: None,
            },
            |ctx, _p| {
                Box::pin(async move {
                    // 10 chunks of 20s: well past the 30s progress window in
                    // total, but never 30s without a checkpoint.
                    for i in 0..10 {
                        ctx.checkpoint().await?;
                        tokio::time::sleep(Duration::from_secs(20)).await;
                        ctx.report(i + 1, Some(10));
                    }
                    Ok(Outcome::success("finished", Some(10)))
                })
            },
        );

        let report = execute(
            &job,
            &run,
            serde_json::json!({}),
            &store,
            CancellationToken::new(),
            None,
        )
        .await;
        assert_eq!(report.state, JobState::Succeeded);
    }

    /// The hard ceiling applies even to a body that is reporting progress.
    #[tokio::test(start_paused = true)]
    async fn max_total_caps_even_a_progressing_body() {
        let store = store();
        let run = queued();
        store.insert(run.clone()).unwrap();
        let job = job_with(
            RetryPolicy::Never,
            TimeoutPolicy {
                no_progress_for_secs: None,
                max_total_secs: Some(60),
            },
            |ctx, _p| {
                Box::pin(async move {
                    for i in 0..100 {
                        ctx.checkpoint().await?;
                        tokio::time::sleep(Duration::from_secs(10)).await;
                        ctx.report(i, Some(100));
                    }
                    Ok(Outcome::ok())
                })
            },
        );

        let report = execute(
            &job,
            &run,
            serde_json::json!({}),
            &store,
            CancellationToken::new(),
            None,
        )
        .await;
        assert_eq!(report.state, JobState::TimedOut);
    }
}
