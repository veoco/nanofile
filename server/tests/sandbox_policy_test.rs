//! What happens when the host cannot give what `sandbox.min_level` asks for.
//!
//! Its own binary because the worker's executable and the requirement are
//! process-global; Linux only because the worker here is a shell script, and
//! macOS applies the real Seatbelt profile to it — which grants the child's own
//! image, not the `/bin/sh` its shebang needs. The decision's *place* is the test.
//!
//! Every case lives in one `#[test]` because the fixture is process-global: the
//! fake worker is installed once per binary, so each report this file measures
//! has to come from the one call that installed it. The script takes its report
//! from a mode file the test writes, which is why the mode is a file and not an
//! environment variable — a child's environment is cleared before it starts.
#![cfg(target_os = "linux")]

mod common;

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use server::indexer::extract::Plan;
use server::sandbox::worker::{self, Outcome, Status};
use server::sandbox::{Level, Profile, Requirement};

/// A worker whose report is chosen by the mode file, and which leaves a mark
/// behind whenever it is started for a request rather than for a probe.
fn scripted_worker(directory: &Path) -> (PathBuf, PathBuf) {
    let marker = common::sandbox::marker_file(directory, "a-document-was-spawned");
    let mode = directory.join("mode");
    common::sandbox::FakeWorker::install(
        directory,
        "policy-worker.sh",
        &format!(
            r#"#!/bin/sh
case "$*" in
  *--selftest*)
    case "$(cat "{mode}" 2>/dev/null)" in
      limits)
        printf '%s\n' "NFS2-sandbox profile=documents level=limits limits=on files=open network=open process=denied detail=fake"
        ;;
      unbounded)
        printf '%s\n' "NFS2-sandbox profile=documents level=limits limits=off files=denied network=denied process=denied detail=fake"
        ;;
      *)
        printf '%s\n' "NFS2-sandbox profile=documents level=partial limits=on files=denied network=open process=open detail=fake"
        ;;
    esac
    exit 0
    ;;
esac
: > "{marker}"
exit 0
"#,
            mode = mode.display(),
            marker = marker.display()
        ),
    );
    (mode, marker)
}

/// Write the report the next probe should produce.
fn set_mode(mode: &Path, value: &str) {
    fs::write(mode, value).expect("write the mode file");
}

#[test]
fn the_minimum_decides_and_the_resource_bound_is_an_invariant() {
    let directory = tempfile::tempdir().expect("temp dir");
    let (mode, marker) = scripted_worker(directory.path());
    let profile = Profile::Documents;

    // ── A host below the minimum is refused before any child ──────────────
    //
    // The script's default report is `partial`: the resource bound and no files
    // layer, which is what a Windows host without a container gives.
    set_mode(&mode, "");
    worker::configure_requirement(Requirement::new(true, Level::Full, true));

    let status = worker::status(profile);
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
        "a host below the minimum is an environment problem, not a verdict: {outcome:?}"
    );
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "the refusal is decided from the cached probe, not from a spawn"
    );
    assert!(
        !marker.exists(),
        "the parent must not hand a request to a worker the requirement refuses"
    );

    // ── A bound host without the files layer is a policy answer ───────────
    //
    // The same host, with the minimum lowered to the bound: the feature runs
    // rather than being disabled over a layer that does not bound memory or CPU.
    set_mode(&mode, "limits");
    worker::configure_requirement(Requirement::new(true, Level::Limits, false));

    let status = worker::status(profile);
    let Status::Ready(report) = &status else {
        panic!("`limits` must accept a host that gives the resource bound: {status:?}");
    };
    assert_eq!(report.level(), Level::Limits);
    assert!(
        report.protections.limits && !report.protections.files,
        "the fixture is the bound without the layer: {}",
        report.detail
    );

    // The request is handed to the child — the fixture does not speak the reply
    // protocol, so the outcome is a failure about the *bytes*; had the
    // requirement refused this host, there would have been no child at all.
    let before = fs::metadata(&marker).is_ok();
    let outcome = worker::extract(Plan::Text, b"irrelevant".to_vec());
    assert!(
        !matches!(outcome, Outcome::Unavailable(_)),
        "a bounded host must not be refused: {outcome:?}"
    );
    assert!(
        !before,
        "the marker has to start absent, or the assertion below proves nothing"
    );
    assert!(
        marker.exists(),
        "the accepted request has to reach the child"
    );

    // ── No bound at all is refused whatever the minimum says ──────────────
    //
    // Every confinement layer in place and no memory or CPU limit: the one shape
    // no setting may run, because a parser with no bound is the failure this
    // module exists to prevent.
    fs::remove_file(&marker).expect("clear the request mark");
    set_mode(&mode, "unbounded");
    worker::configure_requirement(Requirement::new(true, Level::Limits, false));

    let status = worker::status(profile);
    let Status::Unavailable(reason) = &status else {
        panic!("an unbounded child must never be accepted: {status:?}");
    };
    assert!(
        reason.contains("memory") && reason.contains("CPU"),
        "the reason has to say what is missing: {reason}"
    );

    let started = Instant::now();
    let outcome = worker::extract(Plan::Text, b"irrelevant".to_vec());
    assert!(
        matches!(outcome, Outcome::Unavailable(_)),
        "an unbounded host is an environment problem, not a verdict: {outcome:?}"
    );
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "the refusal is decided from the cached probe, not from a spawn"
    );
    assert!(
        !marker.exists(),
        "no request may reach a worker that established no bound"
    );
}
