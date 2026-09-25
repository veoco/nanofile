//! End-to-end acceptance test for load-aware background work.
//!
//! The unit tests cover the gate on its own. This one goes through the real
//! router: a real request holds the foreground gauge, a real registered job
//! checks in at its chunks, and the requests that arrive while that job is
//! running are timed.

mod common;

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use common::TestFixture;
use server::tasks::admission::LoadThresholds;
use server::tasks::registry::RegisteredJob;
use server::tasks::run::{JobState, Outcome, Params, Viewer};
use server::tasks::spec::{
    ChunkPolicy, Dedup, Durability, History, JobKey, JobSpec, Priority, Resource, RetryPolicy,
    TimeoutPolicy, Trigger, Visibility,
};
use tokio::io::AsyncWriteExt;

/// One chunk of the synthetic job's burn, and how many there are. Long in
/// total, so a job that did *not* step aside would be unmistakable.
const CHUNK: Duration = Duration::from_millis(10);
const CHUNKS: u64 = 200;

/// The load generator: how many clients, and how many requests each makes.
const CLIENTS: usize = 12;
const REQUESTS_PER_CLIENT: usize = 20;

/// The bound the interactive requests must stay under while the job runs.
///
/// Deliberately loose. It is not here to pin a number a busy machine can drift
/// past, but to fail if something makes the request path wait on the job.
const P99_BOUND: Duration = Duration::from_millis(500);

/// A request head plus part of a body, and never the rest.
///
/// The server is still reading this body — and so still counting the request
/// as in flight — for as long as the connection stays open. That is what makes
/// "the server is busy" a fact the test controls rather than a race.
const STALLED_REQUEST: &[u8] = b"POST /api2/auth-token/ HTTP/1.1\r\n\
Host: localhost\r\n\
Content-Type: application/json\r\n\
Content-Length: 512\r\n\
\r\n\
{\"username\":\"someone@example.com\",\"password\":\"unfinis";

/// Burn real CPU without yielding to the runtime.
///
/// Deliberately synchronous: a heavy background pass competes for the same
/// runtime the server's requests run on, and this reproduces that.
fn burn(duration: Duration) {
    let deadline = Instant::now() + duration;
    let mut x = 0u64;
    while Instant::now() < deadline {
        x = x
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
    }
    std::hint::black_box(x);
}

/// A job that burns CPU in chunks it can be interrupted between.
fn heavy_spec() -> JobSpec {
    JobSpec {
        key: JobKey::GarbageCollection,
        name: "load-test-heavy",
        trigger: Trigger::Manual,
        priority: Priority::Background,
        resource: Resource::Cpu,
        max_concurrent: 1,
        queue_depth: 0,
        timeout: TimeoutPolicy {
            no_progress_for_secs: None,
            max_total_secs: None,
        },
        retry: RetryPolicy::Never,
        dedup: Dedup::None,
        history: History::EveryRun,
        visibility: Visibility::OwnerOrAdmin,
        durability: Durability::Memory,
        resumable: true,
        cancellable: true,
        chunkable: Some(ChunkPolicy {
            max_chunk_ms: CHUNK.as_millis() as u64,
            unit: "chunk",
        }),
        force_safe: true,
        quiet: None,
        idempotent: true,
    }
}

fn heavy_body() -> server::tasks::spec::JobRunFn {
    Arc::new(|ctx, _params: Params| {
        Box::pin(async move {
            for i in 0..CHUNKS {
                ctx.checkpoint().await?;
                burn(CHUNK);
                ctx.report(i + 1, Some(CHUNKS));
            }
            Ok(Outcome::success("burned", Some(CHUNKS)))
        })
    })
}

/// Poll `predicate` until it holds or `timeout` elapses.
async fn wait_for<F, Fut>(timeout: Duration, mut predicate: F) -> bool
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = bool>,
{
    let deadline = Instant::now() + timeout;
    loop {
        if predicate().await {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

/// The nearest-rank percentile of `samples`.
fn percentile(samples: &[Duration], fraction: f64) -> Duration {
    assert!(!samples.is_empty(), "no request was timed");
    let mut sorted = samples.to_vec();
    sorted.sort_unstable();
    let rank = ((sorted.len() as f64 * fraction).ceil() as usize).clamp(1, sorted.len());
    sorted[rank - 1]
}

/// While a heavy job is running, interactive requests keep answering: the job
/// parks at its checkpoints instead of holding the runtime they need.
#[tokio::test]
async fn interactive_requests_stay_responsive_while_a_heavy_job_runs() {
    let f = TestFixture::new().await;
    let tasks = &f.server.state.tasks;

    tasks
        .register(RegisteredJob::new(heavy_spec(), heavy_body()))
        .expect("the synthetic job registers");

    // Load awareness on, with a threshold a single in-flight request exceeds.
    tasks.configure_load(true, 1);
    tasks.set_load_thresholds(LoadThresholds {
        max_inflight_requests: 0,
        max_db_utilization: 1.0,
        max_worker_busy_pct: 100,
    });

    // Put one real request in flight and leave it there.
    let mut stalled =
        tokio::net::TcpStream::connect(f.server.base_url.trim_start_matches("http://"))
            .await
            .expect("the test server accepts connections");
    stalled.write_all(STALLED_REQUEST).await.unwrap();
    stalled.flush().await.unwrap();
    assert!(
        wait_for(Duration::from_secs(5), || async {
            tasks.load_snapshot().inflight_requests > 0
        })
        .await,
        "the stalled request should be counted as foreground load"
    );

    let run_id = tasks
        .submit(
            JobKey::GarbageCollection,
            Some(f.user_id),
            serde_json::json!({}),
            "load test",
            None,
        )
        .await
        .expect("the run is accepted");

    // It parks before touching any of its work.
    assert!(
        wait_for(Duration::from_secs(5), || async {
            tasks.store().get(&run_id).map(|run| run.state) == Some(JobState::Yielded)
        })
        .await,
        "a busy server must park the heavy job"
    );
    assert_eq!(tasks.store().get(&run_id).unwrap().progress.done, 0);

    // What a client actually sees while that job is running.
    let client = Arc::new(f.server.client());
    let samples = Arc::new(Mutex::new(Vec::new()));
    let mut load = Vec::new();
    for _ in 0..CLIENTS {
        let client = client.clone();
        let samples = samples.clone();
        load.push(tokio::spawn(async move {
            for _ in 0..REQUESTS_PER_CLIENT {
                let started = Instant::now();
                let response = client.get("/api2/ping/", None).await;
                assert_eq!(response.status(), 200);
                samples.lock().unwrap().push(started.elapsed());
            }
        }));
    }
    for task in load {
        task.await.unwrap();
    }

    let p99 = percentile(&samples.lock().unwrap(), 0.99);
    assert!(
        p99 <= P99_BOUND,
        "p99 while a heavy job runs was {p99:?}, over the {P99_BOUND:?} bound"
    );

    // Not merely slow — stopped. The parked job made no progress at all while
    // the server was answering those requests.
    assert_eq!(
        tasks.store().get(&run_id).unwrap().progress.done,
        0,
        "the parked job must not have advanced during the load"
    );

    // Release the server, and the job picks up where it left off: parking held
    // it back rather than ending it.
    drop(stalled);
    assert!(
        wait_for(Duration::from_secs(10), || async {
            tasks
                .store()
                .get(&run_id)
                .is_some_and(|run| run.progress.done > 0)
        })
        .await,
        "the job must resume once the server is quiet"
    );

    // Stopped rather than waited out: the burn is deliberately long.
    tasks.cancel(&run_id, Viewer::user(f.user_id)).unwrap();
    assert!(
        wait_for(Duration::from_secs(10), || async {
            tasks
                .store()
                .get(&run_id)
                .is_some_and(|run| run.state.is_terminal())
        })
        .await,
        "the cancelled run must reach a terminal state"
    );
    assert_eq!(
        tasks.store().get(&run_id).unwrap().state,
        JobState::Cancelled
    );
}
