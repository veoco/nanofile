//! What the parent remembers about a host it has already asked.
//!
//! Its own binary because the worker's executable is process-global; Linux only
//! because the worker here is a shell script, and macOS applies the real Seatbelt
//! profile to it — which grants the child's own image, not the `/bin/sh` its
//! shebang needs. The host is asked once per profile and per requirement.
#![cfg(target_os = "linux")]

mod common;

use std::path::Path;

use server::sandbox::worker::{self, Status};
use server::sandbox::{Profile, Protections, Requirement};

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
    printf '%s\n' "NFS2-sandbox profile=images limits=on files=denied network=open process=open detail=fake"
    ;;
  *--profile\ media*)
    # The wrong profile on purpose: this probe asked for media and the child
    # answers for documents.
    printf '%s\n' "NFS2-sandbox profile=documents limits=on files=denied network=denied process=denied detail=fake"
    ;;
  *)
    printf '%s\n' "NFS2-sandbox profile=documents limits=on files=denied network=denied process=denied detail=fake"
    ;;
esac
exit 0
"#,
            asked = asked.display(),
        ),
    );
    (worker, asked)
}

fn starts(asked: &Path) -> usize {
    common::sandbox::lines(asked)
}

/// The items one profile's cached answer holds, or `None` when it holds none.
fn items(profile: Profile) -> Option<Protections> {
    match worker::status(profile) {
        Status::Ready(report) => Some(report.protections),
        Status::Unavailable(_) => None,
    }
}

fn every_item() -> Protections {
    Protections {
        limits: true,
        files: true,
        network: true,
        process: true,
    }
}

/// network or process item.
fn files_confined() -> Protections {
    Protections {
        limits: true,
        files: true,
        ..Protections::default()
    }
}

#[test]
fn a_probe_is_asked_once_and_its_answer_is_kept_per_profile() {
    let directory = tempfile::tempdir().expect("temp dir");
    let (_worker, asked) = script(directory.path());

    assert_eq!(items(Profile::Documents), Some(every_item()));
    assert_eq!(starts(&asked), 1, "the first call probes");
    assert_eq!(items(Profile::Documents), Some(every_item()));
    assert_eq!(starts(&asked), 1, "a second call is the cached answer");

    let Status::Unavailable(why) = worker::status(Profile::Media) else {
        panic!("a report for documents must not be read as the media probe's");
    };
    assert!(
        why.contains("reported profile=documents"),
        "the reason has to name what came back: {why}"
    );
    assert_eq!(starts(&asked), 2);

    // its own probe gave.
    assert_eq!(items(Profile::Documents), Some(every_item()));
    assert_eq!(
        starts(&asked),
        2,
        "a refused report must not have replaced a stored one"
    );

    // shared entry would have answered `full` here.
    assert_eq!(items(Profile::Images), Some(files_confined()));
    assert_eq!(starts(&asked), 3);

    // An answer belongs to the requirement it was measured under. Flipping the
    // switch makes the stored one unusable, so the host is asked again — and this
    // time the answer is refused. Two mechanisms enforce that and the assertion is
    // about the outcome: a settings change clears the cache, and a stored answer
    // carries the requirement it was measured under so a probe in flight across a
    // change cannot be published under the new one.
    worker::configure_requirement(Requirement::new(false));
    let Status::Unavailable(why) = worker::status(Profile::Images) else {
        panic!("a switched-off sandbox must refuse whatever the host gives");
    };
    assert!(
        why.contains("switched off"),
        "the reason has to name the switch: {why}"
    );
    assert_eq!(starts(&asked), 4, "a new requirement has to re-probe");

    // answers from the record without starting a child.
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

    // ones were taken under the previous requirement.
    worker::configure_requirement(Requirement::new(true));
    assert_eq!(items(Profile::Images), Some(files_confined()));
    assert_eq!(items(Profile::Documents), Some(every_item()));
    assert_eq!(starts(&asked), 6);
}
