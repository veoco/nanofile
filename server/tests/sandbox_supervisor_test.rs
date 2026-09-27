//! What the parent does with a child that does not answer, answers too much,
//! or was started with the wrong surroundings.
//!
//! Its own integration-test binary because the worker's executable is
//! process-global and set once: every other test's fixture points it at the
//! real binary and then insists the sandbox confines it. Here it points at a
//! script that answers the startup self-test like a confined worker and then
//! does the one thing under test.
//!
//! One test rather than several for the same reason `index_worker_wedge_test`
//! is one: the executable is set once per process, and the phases below are
//! three faces of one question — what the parent bounds about the child that it,
//! and only it, can see. A request that never comes back has to end at the
//! deadline; the process group the child leads has to be killed with it, or a
//! copy keeps the protocol pipes; a reply past the cap has to be no reply at
//! all; and the child has to be started outside the server's directory and
//! without its environment.
#![cfg(unix)]

use std::path::Path;
use std::time::{Duration, Instant};

use server::indexer::extract::{MAX_INDEXED_CONTENT_BYTES, Plan, reason};
use server::sandbox::worker::{self, RunOutcome};
use server::sandbox::{Grants, Profile};

/// The script and the files it leaves behind.
struct Worker {
    executable: std::path::PathBuf,
    /// What the child saw of its surroundings: its working directory, then its
    /// environment.
    facts: std::path::PathBuf,
    /// One line per beat of the copy the child leaves in its process group.
    beats: std::path::PathBuf,
    /// The reply the flood branch wrote before it sent it.
    flooded: std::path::PathBuf,
}

/// Most bytes of a reply the parent will read.
///
/// Spelled here from the same two numbers the worker's own cap is built from
/// (`indexer::extract::MAX_INDEXED_CONTENT_BYTES` plus a mebibyte of slack),
/// because the constant itself is private to the module that reads it.
fn reply_cap() -> u64 {
    (MAX_INDEXED_CONTENT_BYTES + (1 << 20)) as u64
}

/// A worker that reports a confinement it does not have, then hangs — or, for
/// the images profile, floods its reply.
fn script(directory: &Path) -> Worker {
    use std::os::unix::fs::PermissionsExt;

    let worker = Worker {
        executable: directory.join("supervisor-worker.sh"),
        facts: directory.join("child-facts"),
        beats: directory.join("copy-beats"),
        flooded: directory.join("flooded-reply"),
    };
    // The parent sets no `PATH`, so the shell finds the programs below the way
    // it finds any program with an empty environment: its own default.
    std::fs::write(
        &worker.executable,
        format!(
            r#"#!/bin/sh
case "$*" in
  *--selftest*)
    profile=documents
    case "$*" in *--profile\ images*) profile=images;; esac
    printf 'NFS2-sandbox profile=%s level=full limits=on files=denied network=denied process=denied detail=fake\n' "$profile"
    exit 0
    ;;
esac
case "$*" in
  *--profile\ images*)
    # Exactly the cap, into a file first: writing it straight to the reply pipe
    # would block once the parent stopped reading, and a blocked child is a
    # timeout rather than the over-long reply this is about.
    dd if=/dev/zero of="{flooded}" bs=1024 count={blocks} 2>/dev/null
    cat "{flooded}"
    exit 0
    ;;
esac
{{
  pwd
  echo "---"
  env
}} > "{facts}"
# A copy in the child's own process group: killing the leader alone would leave
# it running, and it is the copy that would hold the protocol pipes.
i=0
while [ $i -lt 200 ]; do
  echo tick >> "{beats}"
  sleep 0.1
  i=$((i + 1))
done &
sleep 300
"#,
            flooded = worker.flooded.display(),
            blocks = reply_cap() / 1024,
            facts = worker.facts.display(),
            beats = worker.beats.display(),
        ),
    )
    .expect("write the fake worker");
    std::fs::set_permissions(&worker.executable, std::fs::Permissions::from_mode(0o755))
        .expect("make it executable");
    worker
}

/// One request, framed the way the parent frames it — the bytes are not what
/// this file is about, so they never reach a parser.
fn a_request() -> Vec<u8> {
    let mut request = Vec::from(b"NFX1" as &[u8]);
    request.push(Plan::Text.tag());
    request.extend_from_slice(b"irrelevant");
    request
}

/// How many beats the child's copy has written.
fn beats(path: &Path) -> usize {
    std::fs::read_to_string(path)
        .map(|text| text.lines().count())
        .unwrap_or(0)
}

/// One run of the hanging child, and how long it took.
fn a_hanging_request(timeout: Duration) -> (RunOutcome, Duration) {
    let started = Instant::now();
    let outcome = worker::run_request(Profile::Documents, timeout, a_request(), Grants::default());
    (outcome, started.elapsed())
}

#[test]
fn the_parent_bounds_what_the_child_does_and_sees() {
    let directory = tempfile::tempdir().expect("temp dir");
    let worker = script(directory.path());
    assert!(
        worker::configure_executable(worker.executable.clone()),
        "this file must be the first to configure the worker"
    );

    // ── The child's surroundings ────────────────────────────────────────
    let (outcome, elapsed) = a_hanging_request(Duration::from_millis(500));
    assert!(
        matches!(outcome, RunOutcome::Failed(why) if why == reason::TIMED_OUT),
        "a child that never answers is the request's failure, not the host's"
    );
    assert!(
        elapsed < Duration::from_secs(10),
        "the deadline is what ends the wait, not the child: {elapsed:?}"
    );

    let facts = std::fs::read_to_string(&worker.facts).expect("the child recorded its facts");
    let mut lines = facts.lines();
    assert_eq!(
        lines.next(),
        Some("/"),
        "the server's directory is not the child's business"
    );
    // The variables the server itself holds: the child's environment is what
    // the parent sets and nothing else, because that is where the deployment's
    // secrets and paths live.
    for name in ["PATH=", "HOME=", "LD_LIBRARY_PATH=", "NANOFILE_CONFIG="] {
        assert!(
            !facts.contains(&format!("\n{name}")),
            "{name} must not reach the child: {facts}"
        );
    }
    assert!(
        facts.contains("RUST_BACKTRACE=0"),
        "the one variable the parent sets has to be there: {facts}"
    );

    // ── The process group ──────────────────────────────────────────────
    // The copy was running while the request did: without this the assertion
    // below would pass on a script that never started one.
    let (_, _) = a_hanging_request(Duration::from_millis(1000));
    let before = beats(&worker.beats);
    assert!(
        before > 0,
        "the worker's copy must have been running before the deadline"
    );
    std::thread::sleep(Duration::from_millis(600));
    assert_eq!(
        beats(&worker.beats),
        before,
        "the process group has to be killed with the child, or a copy keeps \
         running with the protocol pipes"
    );

    // ── The reply cap ──────────────────────────────────────────────────
    let outcome = worker::run_request(
        Profile::Images,
        Duration::from_secs(20),
        a_request(),
        Grants::default(),
    );
    // The harness first: a worker that never wrote the reply would make the
    // assertion below pass for the wrong reason.
    let flooded = std::fs::metadata(&worker.flooded)
        .expect("the worker wrote the reply it was asked for")
        .len();
    assert_eq!(
        flooded,
        reply_cap(),
        "the reply has to be exactly the cap for this to be about the cap"
    );
    assert!(
        matches!(outcome, RunOutcome::Failed(why) if why == reason::FAILED),
        "a reply the parent cannot frame is a failed document"
    );
}
