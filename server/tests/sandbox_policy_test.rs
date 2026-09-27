//! What happens when the host cannot give what `sandbox.min_level` asks for.
//!
//! Its own binary because the worker's executable and the requirement are
//! process-global; Linux only because the worker here is a shell script, and
//! macOS applies the real Seatbelt profile to it — which grants the child's own
//! image, not the `/bin/sh` its shebang needs. The decision's *place* is the test.
#![cfg(target_os = "linux")]

mod common;

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use server::indexer::extract::Plan;
use server::sandbox::worker::{self, Outcome, Status};
use server::sandbox::{Level, Requirement};

/// A worker that reports a confinement `full` does not accept, and leaves a
/// mark behind whenever it is started for a request rather than for a probe.
fn short_worker(directory: &Path) -> PathBuf {
    let marker = common::sandbox::marker_file(directory, "a-document-was-spawned");
    common::sandbox::FakeWorker::install(
        directory,
        "short-worker.sh",
        &format!(
            r#"#!/bin/sh
case "$*" in
  *--selftest*)
    printf '%s\n' "NFS2-sandbox profile=documents level=partial limits=on files=denied network=open process=open detail=fake"
    exit 0
    ;;
esac
: > "{marker}"
exit 0
"#,
            marker = marker.display()
        ),
    );
    marker
}

#[test]
fn a_minimum_the_host_cannot_meet_is_decided_before_any_child() {
    let directory = tempfile::tempdir().expect("temp dir");
    let marker = short_worker(directory.path());
    worker::configure_requirement(Requirement::new(true, Level::Full));

    let status = worker::status(server::sandbox::Profile::Documents);
    let Status::Unavailable(reason) = &status else {
        panic!("a `full` minimum must not accept partial confinement: {status:?}");
    };
    assert!(
        reason.contains("full") && reason.contains("partial"),
        "the reason has to name the minimum and what the host gave: {reason}"
    );

    // The document is not indexed, and — the point — no child was started to
    // find that out.
    let started = Instant::now();
    let outcome = worker::extract(Plan::Text, b"irrelevant".to_vec());
    assert!(
        matches!(outcome, Outcome::Unavailable(_)),
        "a host below the minimum is an environment problem, not a verdict"
    );
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "the refusal is decided from the cached probe, not from a spawn"
    );
    assert!(
        !marker.exists(),
        "the parent must not hand a request to a worker the requirement refuses"
    );
}
