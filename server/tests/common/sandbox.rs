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
use server::sandbox::{Level, Profile, Report};

/// The helper the media profile is granted, and the one that writes the clips.
///
/// `NANOFILE_TEST_FFMPEG_PATH` names the binary CI installed. The server would
/// otherwise resolve `ffmpeg` on its own `PATH`, which on Windows and macOS is
/// often a package manager's shim: the media grant reaches the one file the
/// parent names and nothing it starts, so a shim would be measured as its own
/// refused second generation.
pub fn helper() -> &'static str {
    static HELPER: OnceLock<String> = OnceLock::new();
    HELPER.get_or_init(|| {
        std::env::var("NANOFILE_TEST_FFMPEG_PATH").unwrap_or_else(|_| "ffmpeg".to_string())
    })
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

/// Point the worker at this package's binary instead of the test process.
///
/// Returns whether *this* call set it: the value is process-global and set once,
/// so a second call is silently ignored and its caller would then be measuring
/// whatever the first one configured.
pub fn configure_real_worker() -> bool {
    worker::configure_executable(PathBuf::from(env!("CARGO_BIN_EXE_nanofile")))
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
pub fn require_level(profile: Profile, minimum: Level) -> Report {
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
/// `--probe` is the half the server runs at startup: it starts the extraction
/// child with the platform's runner around it and applies the requirement, so
/// this measures the path a document takes rather than the child's own
/// diagnostic. `helper` is the media profile's grant; the other profiles have
/// none. The line is printed as well as returned, because a CI log is where it
/// is read when a runner disagrees with what was expected.
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
