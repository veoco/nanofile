//! What the parent does when the worker leaves a process holding the pipes.
//!
//! Its own binary because the worker's executable is process-global: the script
//! here answers the self-test and exits while something it started keeps the
//! inherited stdout open. Reading until end of file would wait for that process
//! and hold the extraction permit with it, so the pipes get a deadline.
#![cfg(unix)]

mod common;

use std::time::{Duration, Instant};

use server::indexer::extract::Plan;
use server::sandbox::worker::{self, Outcome};
use server::sandbox::{Level, Requirement};

/// Write a shell script that answers the startup self-test like a confined
/// worker and then exits while a background process holds the pipes.
fn wedge_worker(directory: &std::path::Path) {
    common::sandbox::FakeWorker::install(
        directory,
        "wedge-worker.sh",
        // The self-test line is what `probe()` parses; without it the parent
        // would decide the worker is unavailable and never start the run this
        // test is about.
        r#"#!/bin/sh
case "$*" in
  *--selftest*)
    printf '%s\n' "NFS2-sandbox profile=documents level=full limits=on files=denied network=denied process=denied detail=fake text_chars=8388608"
    exit 0
    ;;
esac
# Hold the protocol pipes past this process's own exit.
( sleep 30 ) &
exit 0
"#,
    );
}

#[test]
fn a_process_left_holding_the_pipes_does_not_hold_the_caller() {
    let directory = tempfile::tempdir().expect("temp dir");
    wedge_worker(directory.path());
    // A requirement the fake report satisfies: what is under test is the pipe,
    // not the level.
    worker::configure_requirement(Requirement::new(true, Level::Partial));

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
