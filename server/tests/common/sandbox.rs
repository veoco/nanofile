//! What the sandbox test binaries share.
//!
//! The worker's executable, the requirement it is judged against and the helper
//! it may execute are process-global, so each sandbox test file owns its own
//! binary and configures them before the first fixture. What is the same either
//! way lives here: the helper, the status and probe calls, the report's facts,
//! and the shell script that stands in for the worker.

#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;

use server::sandbox::worker::{self, Status};
use server::sandbox::{Level, Profile, Report, Requirement};

/// The helper the media profile is granted, and the one that writes the clips.
///
/// `NANOFILE_TEST_FFMPEG_PATH` names the binary CI installed; without it the
/// command is resolved on this process's `PATH`, because the media grant names
/// one file and a bare name would be a grant on a relative path. The one file is
/// also why a package manager's shim will not do: the grant reaches the named
/// binary and nothing it starts.
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

/// Skip the test on a development host, fail it on CI.
///
/// A host without the helper can do nothing here, but a green suite that never
/// ran the media pipeline is the failure these tests exist to catch.
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

/// Generate a clip larger than the media head cap, so the thumbnail is built
/// from a prefix of the file rather than the whole upload.
///
/// `v210` is uncompressed, so the size is a steady ~43 MiB at this duration —
/// large enough to clear the head cap, which a compressed fixture at this
/// resolution cannot reach. `faststart` places the container index at the front
/// (the common case) when true, and at the end when false — the latter forces
/// the full-file fallback for non-faststart containers.
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

/// Point the worker at this package's binary instead of the test process.
///
/// Returns whether *this* call set it: the value is process-global and set once,
/// so a second call is silently ignored and its caller would then be measuring
/// whatever the first one configured.
pub fn configure_real_worker() -> bool {
    worker::configure_executable(PathBuf::from(env!("CARGO_BIN_EXE_nanofile")))
}

/// Turn the real worker on, once per test binary, and leave it on.
///
/// Every fixture calls this before it starts a server, and two things are
/// process-global: the executable the children are started from, and the
/// requirement. Both are set once here, and a test that points the worker
/// somewhere else *before* its first fixture keeps its own path — the check is
/// what makes that order-insensitive. Re-storing the switch on every fixture
/// would also undo a test's own setting, because `configure_requirement` clears
/// the container give-up records with it.
pub fn enable_real_worker() {
    static ENABLED: std::sync::OnceLock<()> = std::sync::OnceLock::new();
    ENABLED.get_or_init(|| {
        // A test that has already named a binary owns that decision: pointing the
        // worker at something that cannot run is how the unavailable path is
        // reached, and overwriting it here would make that test measure the real
        // worker instead.
        if !worker::executable_is_configured() {
            configure_real_worker();
        }
        worker::configure_requirement(requirement());
    });
}

/// The requirement every test that runs the real worker measures under.
///
/// The master switch, and nothing else, because that is all the model has: what
/// the child established is read back from its own report rather than demanded
/// by a setting. The grade is therefore whatever the platform installed — `full`
/// on Linux and macOS, `limits` on a Windows host that cannot create an
/// AppContainer — and `the_platform_answer` asserts the mechanism that goes with
/// it.
pub fn requirement() -> Requirement {
    Requirement::new(true)
}

/// The requirement a probe test measures a *parser* profile under.
///
/// The same switch: what these tests assert is that the child bounds itself and
/// still does its job, which is true on every platform and is the one item no
/// host may give up.
pub fn configure_probe_requirement() -> Requirement {
    requirement()
}

/// The requirement the *media* probe is measured under.
///
/// The same one: the media profile can never carry the process item — it starts
/// the helper by copying itself — and that is now reported rather than refused,
/// so a parser test and a media test measure under the same rule.
pub fn configure_media_probe_requirement() -> Requirement {
    requirement()
}

/// The `Requirement` an integration test wants the features to run under.
///
/// Every test binary that turns the real worker on calls this before its first
/// fixture: what those tests are about is the feature doing its job, and the
/// layers above the resource bound are the platform's answer rather than a test
/// result. The grades themselves are asserted in `sandbox_probe_test`.
pub fn limits_requirement() -> Requirement {
    requirement()
}

/// Serialises a requirement change with the probe that reads it.
///
/// The requirement is process-global and the tests in one binary run in
/// parallel, so a test that stored its requirement and then probed could have
/// another test's store land in between — and the child would then be started
/// under a *different* requirement than the one the caller asked about, with the
/// report judged against it. That is what a Windows run showed: a probe whose
/// caller wanted no container came back with the container creation's label.
///
/// One lock for the whole binary, and everything that both stores a requirement
/// and reads what it produced goes through [`with_requirement`].
pub fn requirement_lock() -> &'static std::sync::Mutex<()> {
    static LOCK: std::sync::OnceLock<std::sync::Mutex<()>> = std::sync::OnceLock::new();
    LOCK.get_or_init(|| std::sync::Mutex::new(()))
}

/// Store `requirement`, run `probe`, and put the lock back.
///
/// The mutex is held across both, which is the point: a probe is the only reader
/// of the requirement that spans a process boundary, and the store only matters
/// because of it.
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

/// The report for `profile`, which has to grade at least `minimum`.
///
/// The requirement is set here, because a caller that has to *reach* a grade is
/// also the caller that knows which one it wants: a Windows host that gives the
/// resource bound without a container has to be able to reach `limits` without
/// the process-global switch deciding the question first.
pub fn require_level(profile: Profile, minimum: Level) -> Report {
    worker::configure_requirement(requirement());
    let report = status(profile);
    assert!(
        report.level() >= minimum,
        "the {} sandbox must reach {}: {}",
        profile.as_str(),
        minimum.as_str(),
        report.detail
    );
    report
}

/// Run the worker's own startup probe in a child process, and parse its line.
///
/// `--probe` is the half the server runs at startup; `helper` is the media
/// profile's grant, and the other profiles have none. The line is printed as
/// well as returned: a CI log is where it is read when a runner disagrees.
///
/// `--probe` is a *parent*: the switch it stores is the one it judges the worker
/// child it starts by, and the child's report is accepted on the resource bound
/// alone, so this binary measures the same host the server does.
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
    /// Write `body` as an executable script and install it as this process's
    /// worker.
    ///
    /// The executable is process-global and set once, so its caller has to be
    /// the first thing in its binary to reach a fixture — which is why each of
    /// these files is a binary of its own.
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
