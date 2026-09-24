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

/// What the login entry looks like right now.
///
/// Read once (one registry/plist/desktop-file read) and handed to the policy
/// that decides what the tray should show and do.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct LoginState {
    /// A registration exists under our name.
    pub present: bool,
    /// It launches *this* installation (same executable).
    pub ours: bool,
    /// The location it names no longer exists.
    pub stale: bool,
}

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

    /// Everything about the entry, from a single read.
    ///
    /// An entry that exists but cannot be read back as an `(executable, config)`
    /// pair counts as present, is nobody's ("not ours"), and is never reported
    /// as stale — the two answers that could otherwise rewrite or remove
    /// something a person wrote by hand.
    fn state(&self) -> LoginState {
        let recorded = self.recorded();
        LoginState {
            present: recorded.is_some() || self.is_enabled(),
            ours: recorded
                .as_ref()
                .is_some_and(|(exe, _)| entry::same_path(exe, self.exe())),
            stale: recorded
                .as_ref()
                .is_some_and(|(exe, config)| entry::stale_between(exe, config, self.exe())),
        }
    }

    /// Whether the entry launches this installation.
    fn is_ours(&self) -> bool {
        self.state().ours
    }

    /// Remove the entry **only** when it is ours, reporting whether it was.
    ///
    /// Used for the automatic consequences of installing the service: an
    /// elevated helper may be running as a different account, and its `HKCU` is
    /// then somebody else's hive — deleting a login entry there that names a
    /// different copy would be destroying a registration we never wrote.
    fn retire_ours(&self) -> anyhow::Result<bool> {
        if !self.is_ours() {
            return Ok(false);
        }
        self.disable()?;
        Ok(true)
    }
}
