//! How this installation starts without being launched by hand.
//!
//! Login entry (`login`): per-user, `HKCU\…\Run` on Windows, a LaunchAgent on
//! macOS, an XDG autostart entry on Linux. Windows service (`service`):
//! an auto-start Service Control Manager entry, with no session. The two are
//! alternatives on Windows; confirmations and the tray menu live in `crate::tray`.

/// Command-line quoting and path normalization shared by both mechanisms.
pub(crate) mod cmdline;

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
