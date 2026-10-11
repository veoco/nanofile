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

/// Whether `sandbox.windows_container` is to be set for this test binary.
///
/// `NANOFILE_TEST_WINDOWS_CONTAINER=1` (or `true`) asks for it. Windows CI sets
/// neither and therefore measures what a Windows host with no container gives —
/// the limits shape, which is the one this repository ships by default. A
/// runner that *can* create an AppContainer sets the variable to exercise the
/// opt-in path as well.
pub fn windows_container() -> bool {
    std::env::var("NANOFILE_TEST_WINDOWS_CONTAINER")
        .is_ok_and(|value| value == "1" || value.eq_ignore_ascii_case("true"))
}

/// The requirement a probe test measures a *parser* profile under.
///
/// `partial` is what the shipped policy asks for on Linux and macOS, so the
/// grade assertions there stay what they always were. On Windows it asks for the
/// files layer as well, which is why the container is attempted only when
/// `windows_container()` says the runner can give one; otherwise the probe
/// settles for the plain creation and the report says `files=open`.
pub fn configure_probe_requirement() -> Requirement {
    Requirement::from_config(true, "partial", windows_container())
}

/// The requirement the *media* probe is measured under.
///
/// `limits`, because the media profile can never reach `partial`: it starts the
/// helper by copying itself, so it never has the process item, and a `partial`
/// minimum would refuse it on every host and skip the assertion that matters —
/// that the confined child still decodes a frame.
pub fn configure_media_probe_requirement() -> Requirement {
    Requirement::from_config(true, "limits", windows_container())
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
/// The requirement is set here, because a caller that has to *reach* a level is
/// also the caller that knows which level it wants: leaving the process-global
/// requirement at the shipped default would make the host's own grade decide
/// whether the probe is judged at all, and a Windows host that gives the
/// resource bound without a container would come back `unavailable` before the
/// assertion could say why. `windows_container()` still decides whether the
/// files layer is asked for, so a runner that can give a container measures it.
pub fn require_level(profile: Profile, minimum: Level) -> Report {
    worker::configure_requirement(Requirement::from_config(
        true,
        minimum.as_str(),
        windows_container() || minimum > Level::Limits,
    ));
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
