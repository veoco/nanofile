//! The per-user login entry: the registration that starts the desktop instance
//! when somebody logs in.
//!
//! One implementation per platform, behind `LoginEntry`. The registration always
//! records the executable's absolute path and the config path (`--config <absolute
//! path>`), because a login-started process runs with a working directory like
//! `C:\Windows\System32` or `/`, where the default relative `config.toml` is not found.

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

/// What the login entry looks like right now, from reading the registration (and,
/// on Windows, its startup switch), handed to the policy that decides what the tray
/// should show and do.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct LoginState {
    /// A registration exists under our name.
    pub present: bool,
    /// It could be read back as an `(executable, config)` pair.
    pub readable: bool,
    /// It launches *this* installation (same executable).
    pub ours: bool,
    /// The location it names no longer exists.
    pub stale: bool,
    /// The launcher has not switched the entry off (Windows: the startup switch
    /// Task Manager and Settings write). `true` where there is no such switch.
    pub approved: bool,
}

/// Launch-at-login management for the current platform.
pub(crate) trait LoginEntry {
    /// Absolute path of the running executable.
    fn exe(&self) -> &Path;

    /// The `(executable, config file)` pair the entry records, or `None` when
    /// there is no entry or it does not name both: a value that cannot be read
    /// back as a pair is never "repaired" into something the user did not ask for.
    fn recorded(&self) -> Option<(String, String)>;

    fn is_enabled(&self) -> bool;

    /// Whether the platform's own startup switch still lets a login start it.
    /// Windows keeps one (`…\Explorer\StartupApproved\Run`, written by Task Manager
    /// and Settings) that turns the entry off without deleting it; a platform whose
    /// launcher has no such switch answers `true`.
    fn is_approved(&self) -> bool {
        true
    }

    /// Whether a login would actually start this installation: the registration is
    /// there, it is healthy, and it is not switched off behind this menu's back.
    fn starts_at_login(&self) -> bool {
        let state = self.state();
        state.present && !state.stale && state.approved
    }

    fn enable(&self) -> anyhow::Result<()>;

    /// Rewrite the registration for this installation, leaving the launcher's
    /// switch alone: repointing a broken entry must not overrule a user who turned
    /// it off in Task Manager.
    fn repoint(&self) -> anyhow::Result<()> {
        self.enable()
    }

    fn disable(&self) -> anyhow::Result<()>;

    /// Everything about the entry: one that exists but cannot be read back as a
    /// pair is present, is nobody's, and is never reported as stale.
    fn state(&self) -> LoginState {
        let recorded = self.recorded();
        LoginState {
            present: recorded.is_some() || self.is_enabled(),
            readable: recorded.is_some(),
            ours: recorded
                .as_ref()
                .is_some_and(|(exe, _)| entry::same_path(exe, self.exe())),
            stale: recorded
                .as_ref()
                .is_some_and(|(exe, config)| entry::stale_between(exe, config, self.exe())),
            approved: self.is_approved(),
        }
    }

    /// Whether the entry launches this installation.
    fn is_ours(&self) -> bool {
        self.state().ours
    }

    /// Remove the entry **only** when it is ours, reporting whether it was. An
    /// elevated helper may run as a different account, whose `HKCU` is somebody
    /// else's hive: deleting an entry there would destroy a registration we never
    /// wrote.
    fn retire_ours(&self) -> anyhow::Result<bool> {
        if !self.is_ours() {
            return Ok(false);
        }
        self.disable()?;
        Ok(true)
    }
}
