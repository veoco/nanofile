//! What the parent remembers about a host it has already asked.
//!
//! Its own binary because the worker's executable is process-global: here it is
//! a script that answers the self-test, records every invocation, and can answer
//! for the *wrong* profile on purpose. The host is asked once per profile and per
//! requirement, and one answer never stands for another's.
#![cfg(unix)]

mod common;

use std::path::Path;

use server::sandbox::worker::{self, Status};
use server::sandbox::{Level, Profile, Requirement};

/// A worker that reports one profile's confinement, records every start, and
/// answers the media probe with the *documents* report.
fn script(directory: &Path) -> (common::sandbox::FakeWorker, std::path::PathBuf) {
    let asked = common::sandbox::marker_file(directory, "asked");
    let worker = common::sandbox::FakeWorker::install(
        directory,
        "status-worker.sh",
        &format!(
            r#"#!/bin/sh
echo "$*" >> "{asked}"
case "$*" in
  *--selftest*) ;;
  *) exit 0 ;;
esac
case "$*" in
  *--profile\ images*)
    printf '%s\n' "NFS2-sandbox profile=images level=partial limits=on files=denied network=open process=open detail=fake"
    ;;
  *--profile\ media*)
    # The wrong profile on purpose: this probe asked for media and the child
    # answers for documents.
    printf '%s\n' "NFS2-sandbox profile=documents level=full limits=on files=denied network=denied process=denied detail=fake"
    ;;
  *)
    printf '%s\n' "NFS2-sandbox profile=documents level=full limits=on files=denied network=denied process=denied detail=fake"
    ;;
esac
exit 0
"#,
            asked = asked.display(),
        ),
    );
    (worker, asked)
}

/// How many times the child was started, whichever way it was asked.
fn starts(asked: &Path) -> usize {
    common::sandbox::lines(asked)
}

/// The grade one profile's cached answer holds, or `None` when it holds none.
fn grade(profile: Profile) -> Option<Level> {
    match worker::status(profile) {
        Status::Ready(report) => Some(report.level()),
        Status::Unavailable(_) => None,
    }
}

#[test]
fn a_probe_is_asked_once_and_its_answer_is_kept_per_profile() {
    let directory = tempfile::tempdir().expect("temp dir");
    let (_worker, asked) = script(directory.path());

    // A ready answer is cached: the host is asked once, not once per caller.
    assert_eq!(grade(Profile::Documents), Some(Level::Full));
    assert_eq!(starts(&asked), 1, "the first call probes");
    assert_eq!(grade(Profile::Documents), Some(Level::Full));
    assert_eq!(starts(&asked), 1, "a second call is the cached answer");

    // A report for another profile is not this profile's answer.
    let Status::Unavailable(why) = worker::status(Profile::Media) else {
        panic!("a report for documents must not be read as the media probe's");
    };
    assert!(
        why.contains("reported profile=documents"),
        "the reason has to name what came back: {why}"
    );
    assert_eq!(starts(&asked), 2);

    // ... and it did not land in the documents slot, which is still the answer
    // its own probe gave.
    assert_eq!(grade(Profile::Documents), Some(Level::Full));
    assert_eq!(
        starts(&asked),
        2,
        "a refused report must not have replaced a stored one"
    );

    // The profiles hold different powers, so their answers are kept apart: a
    // shared entry would have answered `full` here.
    assert_eq!(grade(Profile::Images), Some(Level::Partial));
    assert_eq!(starts(&asked), 3);

    // An answer belongs to the requirement it was measured under. A new
    // minimum makes the stored one unusable, so the host is asked again — and
    // this time the answer does not meet it. Two mechanisms enforce that and
    // the assertion is about the outcome: a settings change clears the cache,
    // and a stored answer carries the requirement it was measured under so a
    // probe in flight across a change cannot be published under the new one.
    worker::configure_requirement(Requirement::new(true, Level::Full));
    let Status::Unavailable(why) = worker::status(Profile::Images) else {
        panic!("a partial host must not pass a full minimum");
    };
    assert!(
        why.contains("partial") && why.contains("full"),
        "the reason has to name what the host gave and what was asked: {why}"
    );
    assert_eq!(starts(&asked), 4, "a new requirement has to re-probe");

    // The shortfall is not asked about again inside the retry window: the next
    // call answers from the record without starting a child.
    let Status::Unavailable(why) = worker::status(Profile::Images) else {
        panic!("the retry window must answer unavailable");
    };
    assert!(
        why.contains("probe is pending"),
        "the window is what waits, and it says so: {why}"
    );
    assert_eq!(
        starts(&asked),
        4,
        "the retry window must not spawn a child per caller"
    );

    // Under the stricter requirement the documents answer is measured again —
    // its stored one was taken under the old requirement — and meets it.
    assert_eq!(grade(Profile::Documents), Some(Level::Full));
    assert_eq!(starts(&asked), 5);
}
