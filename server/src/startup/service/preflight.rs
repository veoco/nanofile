//! The Windows service preflight: the checks that need Win32, and the entry
//! point that runs them all.
//!
//! A service runs as a different account, in a different working directory and
//! at a different moment (boot, before anybody logs in) than the tray instance
//! that installed it. Every one of those differences turns into a silent failure
//! — a directory it may not write, a per-user drive mapping it cannot see, a log
//! file it cannot open, an `ffmpeg` that is only on the user's `PATH` — and the
//! process that fails has no console and possibly no log file to say so.
//!
//! So the same checks run twice:
//!
//! * **Before installing** (the elevated helper, as the installing
//!   administrator): a refusal is a dialog, while a person is watching.
//! * **When the service starts** (in the service process, as the account that
//!   will serve): the authoritative answer, logged line by line.
//!
//! The report's shape and its platform-independent rules live in
//! [`super::checks`]. The checks read the *configuration file's* values; a
//! directory an administrator changed at `/sysadmin/settings/` is only known
//! once that server starts, which is where `run_server` names the directory it
//! cannot create.

use std::net::TcpListener;
use std::path::{Path, PathBuf};

use infra::config::Config;

use super::checks::{Preflight, Severity, is_sensitive_root, probe_writable, volume_root};

/// Everything a preflight needs, taken from the loaded configuration.
///
/// Owned rather than borrowing `Config` so the tray can keep one and re-run the
/// checks when the menu item is clicked — the configuration it was built from is
/// the one the tray shows (a saved setting reaches the menu at the next start).
#[derive(Clone)]
pub(crate) struct Input {
    config_path: Option<PathBuf>,
    dirs: Vec<(&'static str, PathBuf)>,
    log_dir: Option<PathBuf>,
    ffmpeg_path: String,
    addr: String,
    port: u16,
}

impl Input {
    pub(crate) fn from_config(config: &Config, config_path: &Path) -> Self {
        let log_dir = crate::logging::configured_log_path(config).map(|path| {
            path.parent()
                .filter(|p| !p.as_os_str().is_empty())
                .map(Path::to_path_buf)
                .unwrap_or_else(|| PathBuf::from("."))
        });
        Self {
            config_path: config_path.is_file().then(|| config_path.to_path_buf()),
            dirs: config.state_dirs(),
            log_dir,
            ffmpeg_path: config.storage.ffmpeg_path.clone(),
            addr: config.server.addr.clone(),
            port: config.server.port,
        }
    }

    /// The paths the service account has to be granted access to, in the order
    /// they are granted: every state directory, the log directory, and the
    /// configuration file (which the server writes back to when a new setting
    /// appears). The flag says whether the grant is inheritable — a directory
    /// gets `(OI)(CI)`, a file does not.
    ///
    /// A directory the checks already refused (a system directory, or one on a
    /// drive the service cannot reach) is not granted: the install stops before
    /// this point.
    pub(crate) fn grant_targets(&self) -> Vec<(PathBuf, bool)> {
        let mut targets: Vec<(PathBuf, bool)> = self
            .dirs
            .iter()
            .map(|(_, dir)| (dir.clone(), true))
            .collect();
        if let Some(dir) = &self.log_dir {
            targets.push((dir.clone(), true));
        }
        if let Some(file) = &self.config_path {
            targets.push((file.clone(), false));
        }
        targets
    }
}

/// Run every check against the configuration as it was loaded.
pub(crate) fn run(input: &Input) -> Preflight {
    let mut report = Preflight::default();

    // The configuration file itself: without it the service starts against
    // built-in defaults, on a different port and a different database.
    match &input.config_path {
        Some(path) => match std::fs::File::open(path) {
            Ok(_) => {}
            Err(e) => report.push(
                Severity::Fail,
                "config file",
                format!("{} cannot be read ({e})", path.display()),
            ),
        },
        None => report.push(
            Severity::Warn,
            "config file",
            "no configuration file exists; the service will start with built-in defaults"
                .to_string(),
        ),
    }

    // Every state directory: it has to exist (or be creatable) and be writable
    // by the account the service runs as.
    for (field, dir) in &input.dirs {
        check_dir(&mut report, field, dir);
    }
    // The log file's directory is not a state directory, but for a service it is
    // the only diagnostic channel: `logging::init` falls back to stdout, which a
    // service has no console for.
    if let Some(dir) = &input.log_dir {
        check_dir(&mut report, "logging.file", dir);
    }

    check_temp_dir(&mut report);
    check_ffmpeg(&mut report, &input.ffmpeg_path);
    check_cwd(&mut report, input);
    check_port(&mut report, input);

    report
}

/// One directory: sensitive roots, drive kind, and a write probe.
fn check_dir(report: &mut Preflight, field: &str, dir: &Path) {
    if is_sensitive_root(dir) {
        report.push(
            Severity::Fail,
            field,
            format!(
                "{} is a system directory; point the setting at a directory of its own",
                dir.display()
            ),
        );
        return;
    }

    if let Some(kind) = drive_kind(dir) {
        report.push(
            Severity::Warn,
            field,
            format!(
                "{} is on a {kind}; the service runs as a local account with no per-user drive \
                 mappings and may not reach it",
                dir.display()
            ),
        );
    }

    if let Err(e) = probe_writable(dir) {
        report.push(
            Severity::Fail,
            field,
            format!(
                "{} cannot be created or written ({e}); the service account needs write access",
                dir.display()
            ),
        );
    }
}

/// A remote, removable or unidentifiable drive, described in words.
fn drive_kind(path: &Path) -> Option<&'static str> {
    use windows_sys::Win32::Storage::FileSystem::GetDriveTypeW;
    // The `DRIVE_*` values live in the WindowsProgramming module: an odd home
    // for them, but it is where the SDK puts them.
    use windows_sys::Win32::System::WindowsProgramming::{
        DRIVE_CDROM, DRIVE_FIXED, DRIVE_NO_ROOT_DIR, DRIVE_RAMDISK, DRIVE_REMOTE, DRIVE_REMOVABLE,
        DRIVE_UNKNOWN,
    };

    let root = volume_root(path)?;
    let wide = crate::startup::win32::wide(&root);
    let kind = unsafe { GetDriveTypeW(wide.as_ptr()) };
    match kind {
        DRIVE_FIXED | DRIVE_RAMDISK => None,
        DRIVE_REMOTE => Some("network drive"),
        DRIVE_REMOVABLE => Some("removable drive"),
        DRIVE_CDROM => Some("removable drive"),
        DRIVE_NO_ROOT_DIR => Some("path whose drive does not exist"),
        DRIVE_UNKNOWN => Some("drive of unknown type"),
        _ => Some("drive of an unrecognised type"),
    }
}

/// `%TEMP%` is where a few dependencies may still put scratch files; a service
/// account usually has a writable one, but not always.
fn check_temp_dir(report: &mut Preflight) {
    let dir = std::env::temp_dir();
    if let Err(e) = probe_writable(&dir) {
        report.push(
            Severity::Warn,
            "TEMP",
            format!(
                "{} cannot be written ({e}); operations that use the system temporary directory \
                 may fail under the service account",
                dir.display()
            ),
        );
    }
}

/// `storage.ffmpeg_path` is a command name by default, looked up on `PATH` — and
/// a service inherits the machine `PATH`, not the one a user's session added to.
fn check_ffmpeg(report: &mut Preflight, ffmpeg_path: &str) {
    use windows_sys::Win32::Storage::FileSystem::SearchPathW;

    let value = ffmpeg_path.trim();
    if value.is_empty() {
        return;
    }
    let direct = Path::new(value);
    if direct.is_absolute() {
        if !direct.is_file() {
            report.push(
                Severity::Warn,
                "storage.ffmpeg_path",
                format!(
                    "{} does not exist; audio/video thumbnails will not be generated",
                    direct.display()
                ),
            );
        }
        return;
    }

    let name = crate::startup::win32::wide(value);
    let mut buffer = vec![0u16; 1024];
    let found = unsafe {
        SearchPathW(
            std::ptr::null(),
            name.as_ptr(),
            std::ptr::null(),
            buffer.len() as u32,
            buffer.as_mut_ptr(),
            std::ptr::null_mut(),
        )
    };
    if found == 0 {
        report.push(
            Severity::Warn,
            "storage.ffmpeg_path",
            format!(
                "'{value}' was not found on this process's PATH; a service reads the machine \
                 PATH, so audio/video thumbnails may not be generated"
            ),
        );
        return;
    }
    let end = buffer.iter().position(|c| *c == 0).unwrap_or(buffer.len());
    let resolved = String::from_utf16_lossy(&buffer[..end]);
    if let Ok(profile) = std::env::var("USERPROFILE")
        && !profile.trim().is_empty()
        && resolved.to_lowercase().starts_with(&profile.to_lowercase())
    {
        report.push(
            Severity::Warn,
            "storage.ffmpeg_path",
            format!(
                "'{value}' resolves to {resolved}, inside a user profile; the service account \
                 does not have that PATH entry"
            ),
        );
    }
}

/// A relative state path that was kept as configured is relative to the working
/// directory. For a service that directory is `C:\Windows\System32`, so the state
/// would be nowhere near the installation.
fn check_cwd(report: &mut Preflight, input: &Input) {
    let relative: Vec<String> = input
        .dirs
        .iter()
        .filter(|(_, dir)| dir.is_relative())
        .map(|(field, dir)| format!("{field} ({})", dir.display()))
        .collect();
    if relative.is_empty() {
        return;
    }
    let cwd = std::env::current_dir().unwrap_or_default();
    let exe_dir = infra::common::util::exe_dir().unwrap_or_default();
    if cwd == exe_dir {
        return;
    }
    report.push(
        Severity::Note,
        "state paths",
        format!(
            "{} name a path relative to the working directory; this process runs in {}, so the \
             service would resolve them somewhere else unless the account's working directory is \
             set explicitly",
            relative.join(", "),
            cwd.display()
        ),
    );
}

/// The port the service will bind, described as a note rather than a failure.
///
/// "In use" is expected when the tray that is installing the service is the
/// process serving the address right now, so this never blocks an install.
fn check_port(report: &mut Preflight, input: &Input) {
    let addr = input.addr.trim();
    if addr.is_empty() {
        return;
    }
    let spec = format!("{addr}:{}", input.port);
    if TcpListener::bind(&spec).is_ok() {
        return;
    }
    report.push(
        Severity::Note,
        "server.addr",
        format!(
            "{spec} cannot be bound right now. That is expected while the instance installing \
             the service is the one serving it; if another program holds the port, the service \
             will fail to start at boot and retry three times"
        ),
    );
}
