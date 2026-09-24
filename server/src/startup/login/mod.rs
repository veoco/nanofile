//! The per-user login entry: the registration that starts the desktop instance
//! when somebody logs in.
//!
//! One implementation per platform, behind [`LoginEntry`]. The registration
//! always records the executable's absolute path and the config path
//! (`--config <absolute path>`), because a login-started process runs with a
//! working directory like `C:\Windows\System32` or `/`, where the default
//! relative `config.toml` would not be found.
//!
//! This module is only compiled where the tray is, which is what uses it today.
//! Windows builds without the tray do not carry it yet.

use std::path::Path;

pub(crate) mod entry;

#[cfg(target_os = "windows")]
#[path = "windows.rs"]
mod platform;
#[cfg(target_os = "macos")]
#[path = "macos.rs"]
mod platform;
#[cfg(target_os = "linux")]
#[path = "linux.rs"]
mod platform;
#[cfg(not(any(target_os = "windows", target_os = "macos", target_os = "linux")))]
#[path = "fallback.rs"]
mod platform;

pub(crate) use platform::LoginManager as PlatformLogin;

/// Launch-at-login management for the current platform.
pub(crate) trait LoginEntry {
    /// Absolute path of the running executable.
    fn exe(&self) -> &Path;

    /// The `(executable, config file)` pair the entry records, or `None` when
    /// there is no entry, or it does not name both.
    ///
    /// `None` is what keeps a hand-edited or foreign registration safe: a value
    /// that cannot be read back as a pair is never "repaired" into something the
    /// user did not ask for.
    fn recorded(&self) -> Option<(String, String)>;

    fn is_enabled(&self) -> bool;
    fn enable(&self) -> anyhow::Result<()>;
    fn disable(&self) -> anyhow::Result<()>;

    /// Whether the recorded entry points at a location that no longer exists.
    ///
    /// An entry records absolute paths, so moving or renaming the installation
    /// leaves it launching something that is not there — which fails silently at
    /// login. Reporting that here lets the tray repoint the entry at the copy the
    /// user is actually running. "Unknown, leave it alone" is the answer for a
    /// backend that cannot read its entry.
    fn is_stale(&self) -> bool {
        let Some((exe, config)) = self.recorded() else {
            return false;
        };
        entry::stale_between(&exe, &config, self.exe())
    }
}
