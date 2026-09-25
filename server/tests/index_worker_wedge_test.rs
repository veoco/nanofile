//! What the parent does when the worker leaves a process holding the pipes.
//!
//! Its own integration-test binary for the same reason as
//! `index_worker_unavailable_test`: the worker's executable is process-global
//! and set once, and every other test's fixture points it at the real binary.
//!
//! The behaviour under test is the one a timeout cannot reach: the direct child
//! *exits* — successfully, even — while something it started keeps the inherited
//! stdout open. Reading until end of file would then wait for that process, and
//! the extraction permit the caller holds (see `service::index`) would be held
//! with it: two such documents would stop document extraction for good. The
//! parent gives the pipes a deadline instead, and a run whose reply did not
//! arrive by then is a failed document, which the backfill retries.
#![cfg(unix)]

use std::path::PathBuf;
use std::time::{Duration, Instant};

use server::indexer::extract::worker::{self, Outcome};
use server::indexer::extract::{Plan, sandbox};

/// Write a shell script that answers the startup self-test like a confined
/// worker and then exits while a background process holds the pipes.
fn wedge_worker(directory: &std::path::Path) -> PathBuf {
    use std::os::unix::fs::PermissionsExt;

    let path = directory.join("wedge-worker.sh");
    std::fs::write(
        &path,
        // The self-test line is what `probe()` parses; without it the parent
        // would decide the worker is unavailable and never start the run this
        // test is about.
        r#"#!/bin/sh
case "$*" in
  *--selftest*)
    printf '%s\n' "NFX1-sandbox level=full limits=on files=denied network=denied process=denied closure=full detail=fake text_chars=8388608"
    exit 0
    ;;
esac
# Hold the protocol pipes past this process's own exit.
( sleep 30 ) &
exit 0
"#,
    )
    .expect("write the fake worker");
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755))
        .expect("make it executable");
    path
}

#[test]
fn a_process_left_holding_the_pipes_does_not_hold_the_caller() {
    let directory = tempfile::tempdir().expect("temp dir");
    let executable = wedge_worker(directory.path());
    assert!(
        worker::configure_executable(executable),
        "this file must be the first to configure the worker"
    );
    // A policy that the fake report satisfies: what is under test is the pipe,
    // not the level.
    worker::configure_policy(sandbox::Policy::Prefer);

    let started = Instant::now();
    let outcome = worker::extract(Plan::Text, b"irrelevant".to_vec());
    let elapsed = started.elapsed();

    assert!(
        elapsed < Duration::from_secs(10),
        "the parent must not wait for a process the worker left behind: {elapsed:?}"
    );
    assert!(
        matches!(outcome, Outcome::Failed(_)),
        "a reply that never arrived is a failed document, not a hang"
    );
}
