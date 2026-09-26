//! What happens when the host cannot give what `sandbox.min_level` asks for.
//!
//! Its own integration-test binary because both the worker's executable and the
//! policy are process-global and set once. The worker here is a script that
//! reports *partial* confinement, which is what a Linux host without Landlock,
//! or a Windows host whose AppContainer would not start, looks like from the
//! parent's side.
//!
//! The behaviour under test is the decision's *place*: the parent applies the
//! policy once, from the probe it already runs, so a shortfall is an error at
//! startup rather than a child spawned per document that dies with exit 125.
#![cfg(unix)]

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use server::indexer::extract::Plan;
use server::sandbox::worker::{self, Outcome, Status};
use server::sandbox::{Level, Requirement};

/// A worker that reports a confinement `full` does not accept, and leaves a
/// mark behind whenever it is started for a request rather than for a probe.
fn short_worker(directory: &Path) -> (PathBuf, PathBuf) {
    use std::os::unix::fs::PermissionsExt;

    let marker = directory.join("a-document-was-spawned");
    let path = directory.join("short-worker.sh");
    std::fs::write(
        &path,
        format!(
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
    )
    .expect("write the fake worker");
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755))
        .expect("make it executable");
    (path, marker)
}

#[test]
fn a_minimum_the_host_cannot_meet_is_decided_before_any_child() {
    let directory = tempfile::tempdir().expect("temp dir");
    let (executable, marker) = short_worker(directory.path());
    assert!(
        worker::configure_executable(executable),
        "this file must be the first to configure the worker"
    );
    worker::configure_requirement(Requirement::new(true, Level::Full));

    let status = worker::status();
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
