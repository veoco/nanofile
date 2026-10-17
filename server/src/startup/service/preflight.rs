//! The Windows service preflight: the Win32 checks and their entry point.
//!
//! A service runs as a different account, in a different working directory and
//! at a different moment (boot, before anybody logs in) than the tray instance
//! that installed it, so each difference becomes a silent failure.
//! The same checks run twice: before installing, as the installing administrator
//! (a refusal is a dialog while somebody watches), and when the service starts
//! (the authoritative answer, logged line by line).
//!
//! The report's shape and rules live in [`super::checks`]; values come from the
//! *configuration file*, so a directory changed at `/sysadmin/settings/` is only
//! known once that server starts.

use std::net::TcpListener;
use std::path::{Path, PathBuf};

use infra::config::Config;

use super::checks::{Preflight, Severity, is_sensitive_root, probe_writable, volume_root};

/// Everything a preflight needs, taken from the loaded configuration.
///
/// Owned rather than borrowing `Config` so the tray can re-run the checks from
/// the configuration it shows.
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

    /// The paths the service account is granted access to, in order: every state
    /// directory, the log directory, then the configuration file (the server
    /// writes back to it). The flag marks an inheritable grant: a directory gets
    /// `(OI)(CI)`, a file does not. A refused directory is not granted at all.
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

pub(crate) fn run(input: &Input) -> Preflight {
    let mut report = Preflight::default();

    // Without it the service starts on built-in defaults: another port, another
    // database.
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

    // Each state directory must be creatable and writable by the service account.
    for (field, dir) in &input.dirs {
        check_dir(&mut report, field, dir);
    }
    // Not a state dir, but the service's only diagnostic channel: `logging::init`
    // falls back to stdout, and a service has no console.
    if let Some(dir) = &input.log_dir {
        check_dir(&mut report, "logging.file", dir);
    }

    check_temp_dir(&mut report);
    check_ffmpeg(&mut report, &input.ffmpeg_path);
    check_cwd(&mut report, input);
    check_port(&mut report, input);

    report
}

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
    // The `DRIVE_*` values live in `WindowsProgramming`, an odd home but the SDK's.
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

    // Resolve through the same library call the runtime uses
    // (`sandbox::worker::resolve_helper`), so a helper the media grants will find
    // is not reported missing here; an unresolved name comes back relative.
    let resolved = server::sandbox::worker::resolve_helper(value);
    if !resolved.is_absolute() {
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
    let resolved = resolved.to_string_lossy();
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

/// A relative state path resolves against the working directory. For a service
/// that directory is `C:\Windows\System32`, nowhere near the installation.
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

/// The port the service will bind, as a note rather than a failure: "in use" is
/// expected while the tray installing the service is serving the address.
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
