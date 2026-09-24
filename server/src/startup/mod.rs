//! How this installation starts without being launched by hand.
//!
//! Two mechanisms exist, and on Windows they are alternatives:
//!
//! * **Login entry** ([`login`]) — a per-user registration that starts the
//!   desktop instance when somebody logs in: `HKCU\…\Run` on Windows, a
//!   LaunchAgent on macOS, an XDG autostart entry on Linux.
//! * **Windows service** ([`service`]) — an auto-start Service Control Manager
//!   entry that runs with no session at all.
//!
//! Nothing here knows about the tray: the mechanisms read and write a
//! registration and report what they found. Confirmations, elevation prompts
//! and the menu itself live in `crate::tray`.

/// Windows command-line quoting and path normalization, shared by the login
/// entry and the service registration.
pub(crate) mod cmdline;

/// The per-user login entry, which only the tray reads and writes today.
#[cfg(feature = "tray")]
pub(crate) mod login;

/// The Windows service registration and control loop. The platform-independent
/// part (the command line, the `ImagePath` comparison, the probe types) is
/// compiled everywhere so its tests run on every platform.
pub(crate) mod service;

/// Win32 helpers shared by the mechanisms.
#[cfg(target_os = "windows")]
pub(crate) mod win32;
