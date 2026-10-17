//! What the sandbox test binaries share.
//!
//! The worker executable, requirement and media grant are process-global, so each
//! sandbox test binary configures them before its first fixture; the shared helper,
//! probe calls and fake-worker script live here.

#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;

use server::sandbox::worker::{self, Status};
use server::sandbox::{Profile, Protections, Report, Requirement};

/// The helper the media profile is granted, and the one that writes the clips.
/// `NANOFILE_TEST_FFMPEG_PATH` names the binary CI installed; without it the command
/// is resolved on `PATH`, because the grant names one file and a bare name would be a
/// grant on a relative path. A package manager's shim will not do either: the grant
/// reaches the named binary and nothing it starts.
pub fn helper() -> &'static str {
    static HELPER: OnceLock<String> = OnceLock::new();
    HELPER.get_or_init(|| {
        std::env::var("NANOFILE_TEST_FFMPEG_PATH")
            .ok()
            .or_else(|| on_path("ffmpeg"))
            .unwrap_or_else(|| "ffmpeg".to_string())
    })
}

/// The first file named `name` on `PATH`, if there is one.
fn on_path(name: &str) -> Option<String> {
    std::env::split_paths(&std::env::var_os("PATH")?)
        .map(|directory| directory.join(name))
        .find(|candidate| candidate.is_file())
        .map(|candidate| candidate.to_string_lossy().into_owned())
}

/// Whether the helper runs at all.
pub fn helper_runs() -> bool {
    Command::new(helper())
        .arg("-version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

/// Skip the test on a development host, fail it on CI: a green suite that never ran
/// the media pipeline is the failure these tests exist to catch.
pub fn without_ffmpeg() {
    if std::env::var_os("CI").is_some() {
        panic!(
            "{} is not runnable; CI installs ffmpeg on this platform",
            helper()
        );
    }
    eprintln!(
        "{} is not available; skipping the media sandbox test",
        helper()
    );
}

/// Generate a one-second clip with the helper, or answer false when it cannot.
pub fn generate_clip(path: &Path) -> bool {
    Command::new(helper())
        .args([
            "-y",
            "-loglevel",
            "error",
            "-f",
            "lavfi",
            "-i",
            "testsrc=duration=1:size=320x240:rate=10",
            "-pix_fmt",
            "yuv420p",
        ])
        .arg(path)
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

/// Generate a clip larger than the media head cap, so the thumbnail is built from a
/// prefix of the file rather than the whole upload. `v210` is uncompressed, so the size
/// is a steady ~43 MiB at this duration; `faststart` puts the container index at the
/// front when true and at the end when false (the full-file fallback).
pub fn generate_large_clip(path: &Path, faststart: bool) -> bool {
    let mut args: Vec<&str> = vec![
        "-y",
        "-loglevel",
        "error",
        "-f",
        "lavfi",
        "-i",
        "testsrc=duration=7:size=320x240:rate=30",
        "-c:v",
        "v210",
    ];
    if faststart {
        args.push("-movflags");
        args.push("+faststart");
    }
    args.push(path.to_str().expect("utf8 path"));
    Command::new(helper())
        .args(&args)
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

/// Point the worker at this package's binary instead of the test process. Returns
/// whether *this* call set it: the value is process-global and set once, so a second
/// call is silently ignored.
pub fn configure_real_worker() -> bool {
    worker::configure_executable(PathBuf::from(env!("CARGO_BIN_EXE_nanofile")))
}

/// Turn the real worker on, once per test binary, and leave it on. The executable and
/// the requirement are process-global and both set here; a test that points the worker
/// elsewhere *before* its first fixture keeps its own path, and re-storing the switch
/// would undo it because `configure_requirement` clears the give-up records.
pub fn enable_real_worker() {
    static ENABLED: std::sync::OnceLock<()> = std::sync::OnceLock::new();
    ENABLED.get_or_init(|| {
        // A test that already named a binary owns that decision: the unavailable path
        // is reached by pointing the worker at something that cannot run.
        if !worker::executable_is_configured() {
            configure_real_worker();
        }
        worker::configure_requirement(requirement());
    });
}

/// The requirement every test that runs the real worker measures under: the master
/// switch alone, since that is all the model has. The grade is whatever the platform
/// installed, and `the_platform_answer` asserts the mechanism that goes with it.
pub fn requirement() -> Requirement {
    Requirement::new(true)
}

/// The requirement a probe test measures a *parser* profile under: the same switch.
/// These tests assert that the child bounds itself and still does its job, which is
/// true on every platform.
pub fn configure_probe_requirement() -> Requirement {
    requirement()
}

/// The requirement the *media* probe is measured under: the same one. The media profile
/// cannot carry the process item (it starts the helper by copying itself), and that is
/// reported rather than refused.
pub fn configure_media_probe_requirement() -> Requirement {
    requirement()
}

/// The `Requirement` an integration test wants the features to run under. Called
/// before the first fixture; the grades themselves are asserted in `sandbox_probe_test`.
pub fn limits_requirement() -> Requirement {
    requirement()
}

/// Serialises a requirement change with the probe that reads it. The requirement is
/// process-global and tests run in parallel, so a store could land between another
/// test's store and its probe, judging the child under the wrong requirement. One lock
/// for the binary; every store-and-read goes through [`with_requirement`].
pub fn requirement_lock() -> &'static std::sync::Mutex<()> {
    static LOCK: std::sync::OnceLock<std::sync::Mutex<()>> = std::sync::OnceLock::new();
    LOCK.get_or_init(|| std::sync::Mutex::new(()))
}

/// Store `requirement`, run `probe`, and put the lock back. The mutex is held across
/// both, which is the point: a probe is the only reader that spans a process boundary.
pub fn with_requirement<T>(requirement: Requirement, probe: impl FnOnce() -> T) -> T {
    let guard = requirement_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    worker::configure_requirement(requirement);
    let result = probe();
    drop(guard);
    result
}

/// The report for `profile`, or a panic naming what the host gave instead.
pub fn status(profile: Profile) -> Report {
    match worker::status(profile) {
        Status::Ready(report) => report,
        Status::Unavailable(why) => {
            panic!("the {} sandbox is unavailable: {why}", profile.as_str())
        }
    }
}

/// The report for `profile`, which has to have its resource bound in place. The
/// requirement is set here because a caller that needs a *bounded* child knows the
/// question; a Windows host that gives the bound without a container must be reachable
/// without the process-global switch deciding first.
pub fn require_bound(profile: Profile) -> Report {
    worker::configure_requirement(requirement());
    let report = status(profile);
    assert!(
        report.protections.get(Protections::REQUIRED) == Some(true),
        "the {} sandbox must be bounded: {}",
        profile.as_str(),
        report.detail
    );
    report
}

/// Run the worker's own startup probe in a child process, and parse its line. `--probe`
/// is the half the server runs at startup; `helper` is the media profile's grant, and the
/// other profiles have none. The line is also printed, because a CI log is where it is
/// read when a runner disagrees. `--probe` is a *parent*: the child's report is accepted
/// on the resource bound alone, so this measures the same host the server does.
pub fn probe(profile: Profile, helper: Option<&str>) -> (Report, String) {
    let mut command = Command::new(env!("CARGO_BIN_EXE_nanofile"));
    command.args(["extract-worker", "--probe", "--profile", profile.as_str()]);
    if let Some(helper) = helper {
        command.args(["--ffmpeg", helper]);
    }
    let output = command.output().expect("the worker binary runs");
    assert!(
        output.status.success(),
        "the {} probe failed: {}",
        profile.as_str(),
        String::from_utf8_lossy(&output.stderr).trim()
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    let line = stdout.lines().next().unwrap_or_default().to_string();
    println!("{line}");
    let report = Report::parse(&line).unwrap_or_else(|| panic!("unreadable report: {line}"));
    (report, line)
}

/// One `key=value` fact of a report's detail, if it carries that key.
pub fn fact<'a>(report: &'a Report, key: &str) -> Option<&'a str> {
    report.detail.split(',').find_map(|entry| {
        let (name, value) = entry.split_once('=')?;
        (name == key).then_some(value)
    })
}

/// Whether a report's detail carries `token`, whole.
pub fn has(report: &Report, token: &str) -> bool {
    report.detail.split(',').any(|fact| fact == token)
}

/// A shell script that answers the worker's self-test, and does one thing after.
pub struct FakeWorker {
    /// The script, which is what `configure_executable` is given.
    pub executable: PathBuf,
}

impl FakeWorker {
    /// Write `body` as an executable script and install it as this process's worker.
    /// Its caller must be the first thing in its binary to reach a fixture, because the
    /// executable is process-global and set once.
    pub fn install(directory: &Path, name: &str, body: &str) -> Self {
        let worker = FakeWorker {
            executable: directory.join(name),
        };
        std::fs::write(&worker.executable, body).expect("write the fake worker");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&worker.executable, std::fs::Permissions::from_mode(0o755))
                .expect("make it executable");
        }
        assert!(
            worker::configure_executable(worker.executable.clone()),
            "this file must be the first to configure the worker"
        );
        worker
    }
}

/// A path a fake worker's script can append to, for counting or recording.
pub fn marker_file(directory: &Path, name: &str) -> PathBuf {
    directory.join(name)
}

/// How many lines a marker file holds, or zero when it was never written.
pub fn lines(path: &Path) -> usize {
    std::fs::read_to_string(path)
        .map(|text| text.lines().count())
        .unwrap_or(0)
}
