//! How this installation starts without being launched by hand.
//!
//! Login entry (`login`): per-user, `HKCU\…\Run` on Windows (respecting the
//! startup switch Task Manager and Settings write), a LaunchAgent on macOS, an
//! XDG autostart entry on Linux. Windows service (`service`):
//! an auto-start Service Control Manager entry, with no session. Start at boot
//! implies the login tray; confirmations and the tray menu live in `crate::tray`.

use std::path::{Path, PathBuf};

/// Command-line quoting and path normalization shared by both mechanisms.
pub(crate) mod cmdline;

/// Which role a `server` start takes: serve, tray client, headless, or refuse.
pub(crate) mod role;

/// Canonical absolute path, with Windows `\\?\` verbatim prefixes stripped so the
/// result can be written into a `Run` value or an `ImagePath`. Falls back to
/// cwd-joining for files that do not exist (yet).
pub(crate) fn absolute_path(path: &Path) -> PathBuf {
    let resolved = path.canonicalize().unwrap_or_else(|_| {
        if path.is_absolute() {
            path.to_path_buf()
        } else {
            std::env::current_dir().unwrap_or_default().join(path)
        }
    });
    #[cfg(windows)]
    {
        let s = resolved.to_string_lossy();
        if let Some(rest) = s.strip_prefix(r"\\?\UNC\") {
            PathBuf::from(format!(r"\\{rest}"))
        } else if let Some(rest) = s.strip_prefix(r"\\?\") {
            PathBuf::from(rest)
        } else {
            resolved
        }
    }
    #[cfg(not(windows))]
    resolved
}

/// The per-user login entry.
///
/// Compiled on Windows unconditionally: `nanofile service install` retires the
/// entry there, which without the tray is the only reachable path.
#[cfg(any(target_os = "windows", feature = "tray"))]
#[cfg_attr(not(feature = "tray"), allow(dead_code))]
pub(crate) mod login;

/// The rule that keeps the login entry and the Windows service alternatives.
#[cfg(feature = "tray")]
pub(crate) mod policy;

/// The Windows service registration and control loop. The platform-independent
/// part (command line, `ImagePath` comparison, probe types) is compiled
/// everywhere so its tests run on every platform.
pub(crate) mod service;

/// Win32 helpers shared by the mechanisms.
#[cfg(target_os = "windows")]
pub(crate) mod win32;
