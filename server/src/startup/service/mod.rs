//! Windows service support: running nanofile under the Service Control Manager
//! (SCM), and registering/removing that service.
//!
//! Why a service: the login entry (`HKCU\…\Run`) only starts when somebody logs
//! in. A service starts at boot with no session at all, which is what a
//! file-sync server wants on a machine nobody sits in front of.
//!
//! Shape of this module:
//!
//! * The command line, the comparison against a registered `ImagePath`, the
//!   account the service will run under and the preflight checks are
//!   platform-independent, so their unit tests run everywhere
//!   (`cargo test -p server --bin nanofile`).
//! * Everything that talks to the SCM or the registry is `#[cfg(windows)]`.
//! * The service *registers* itself; it never starts or stops itself. Switching
//!   from a logged-in tray instance to the service therefore takes effect at the
//!   next system start: handing the listening port over immediately is not
//!   reliable on Windows, because a port whose connections were just closed
//!   stays in `TIME_WAIT` for minutes and the new process would fail to bind it.

// The neutral surface below (the command line, the `ImagePath` comparison, the
// probe types) is only *reachable* from a Windows tray build; elsewhere it is
// exercised by the tests. One attribute instead of a `#[allow(dead_code)]` on
// every item — and on the platform it ships for, the lint stays active.
#![cfg_attr(not(all(target_os = "windows", feature = "tray")), allow(dead_code))]

use std::path::Path;

use crate::startup::cmdline::{normalize_win_path, win_cmd_quote};

/// The SCM name of the service. Fixed (not derived from the install path) so
/// `net stop Nanofile`/`sc query Nanofile` are predictable, and so the tray can
/// find the service it manages. One service per machine.
pub(crate) const SERVICE_NAME: &str = "Nanofile";

/// What the service is called in `services.msc`.
pub(crate) const DISPLAY_NAME: &str = "Nanofile Sync Server";

/// What `nanofile service …` can be asked to do.
#[cfg(target_os = "windows")]
#[derive(clap::Subcommand, Debug)]
pub(crate) enum ServiceAction {
    /// Run as a service. Called by the Service Control Manager (the registered
    /// binary path), never by hand: without the SCM this exits immediately.
    Run,
    /// Register the auto-start service (or update its command line). Needs
    /// administrator rights.
    Install,
    /// Stop and remove the service. Needs administrator rights.
    Uninstall,
    /// Report whether the service is registered. Exit code 0 when it is.
    Status,
}

/// What is registered under [`SERVICE_NAME`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ServiceProbe {
    /// No service with our name.
    NotInstalled,
    /// Registered, but its binary path belongs to a different installation
    /// (another copy of nanofile, or another config file).
    ///
    /// `missing` means the executable that path names no longer exists, which is
    /// what a moved or renamed installation leaves behind: the service then
    /// fails to start and nothing on this side says why. The replacement
    /// confirmation names it.
    OtherInstall {
        image_path: String,
        running: bool,
        missing: bool,
    },
    /// Registered for this executable and config file.
    Ours { running: bool },
    /// The SCM could not be asked (rights, or an unexpected error).
    Unknown,
}

impl ServiceProbe {
    pub fn is_ours(&self) -> bool {
        matches!(self, ServiceProbe::Ours { .. })
    }

    pub fn is_running(&self) -> bool {
        match self {
            ServiceProbe::Ours { running } | ServiceProbe::OtherInstall { running, .. } => *running,
            _ => false,
        }
    }

    /// Whether the registered executable is gone (a moved or renamed install).
    #[allow(dead_code)] // asserted by the tests below; the menu only asks `is_ours`
    pub fn is_broken(&self) -> bool {
        match self {
            ServiceProbe::OtherInstall { missing, .. } => *missing,
            _ => false,
        }
    }

    #[allow(dead_code)] // asserted by the tests below; the menu only asks `is_ours`
    pub fn is_installed(&self) -> bool {
        !matches!(self, ServiceProbe::NotInstalled | ServiceProbe::Unknown)
    }
}

// ── Platform-independent helpers ─────────────────────────────────────────────

/// The command line registered as the service's binary path: the same
/// executable, told explicitly which config file to use (a service starts with
/// a working directory of `C:\Windows\System32`, where a relative `config.toml`
/// means nothing).
pub(crate) fn service_command_line(exe: &Path, config: &Path) -> String {
    format!(
        "{} service run --config {}",
        win_cmd_quote(&exe.to_string_lossy()),
        win_cmd_quote(&config.to_string_lossy())
    )
}

/// The service's description in `services.msc`.
pub(crate) fn service_description(config: &Path) -> String {
    format!(
        "Nanofile — a Seafile-compatible file sync server. Starts at boot without a user \
         login. Configuration: {}",
        config.display()
    )
}

/// The executable an `ImagePath` (or any command line of ours) names: the first
/// quoted token when the path is quoted, otherwise everything up to the first
/// space.
///
/// It is only used to ask "does the file this registration launches still
/// exist?", so a hand-edited value that parses oddly can only make the answer
/// less precise — never destructive.
pub(crate) fn executable_in(command_line: &str) -> Option<String> {
    let text = command_line.trim();
    let token = match text.strip_prefix('"') {
        Some(rest) => rest.split('"').next()?,
        None => text.split_whitespace().next()?,
    };
    let token = token.trim();
    (!token.is_empty()).then(|| token.to_string())
}

/// Whether a registered `ImagePath` is the command line this installation would
/// write.
///
/// Not an exact-equality test on purpose: a service registered by hand (or by an
/// older build) may carry fewer arguments, and reporting "someone else's
/// install" for a service that starts exactly this binary would be wrong. What
/// has to differ for the answer to be "not ours" is the executable or the
/// config file, and both show up as a difference in the middle of the string.
pub(crate) fn image_path_matches(registered: &str, expected: &str) -> bool {
    let registered = normalize_win_path(registered);
    let expected = normalize_win_path(expected);
    !registered.is_empty()
        && !expected.is_empty()
        && (registered == expected
            || registered.starts_with(&expected)
            || expected.starts_with(&registered))
}

#[cfg(target_os = "windows")]
mod cli;
#[cfg(target_os = "windows")]
mod windows;

#[cfg(target_os = "windows")]
pub(crate) use cli::run_cli;
#[cfg(all(target_os = "windows", feature = "tray"))]
pub(crate) use windows::probe as probe_service;
#[cfg(all(target_os = "windows", feature = "tray"))]
pub(crate) use windows::stop_service;

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn the_command_line_quotes_both_paths() {
        let line = service_command_line(
            &PathBuf::from(r"C:\Apps\Na nofile\nanofile.exe"),
            &PathBuf::from(r"C:\Apps\Na nofile\config.toml"),
        );
        assert_eq!(
            line,
            r#""C:\Apps\Na nofile\nanofile.exe" service run --config "C:\Apps\Na nofile\config.toml""#
        );
    }

    #[test]
    fn the_command_line_escapes_inner_quotes() {
        let line = service_command_line(
            &PathBuf::from(r#"C:\we"ird\nanofile.exe"#),
            &PathBuf::from(r"C:\cfg.toml"),
        );
        assert!(line.contains(r#""C:\we\"ird\nanofile.exe""#));
    }

    #[test]
    fn the_description_names_the_config_file() {
        let described = service_description(&PathBuf::from(r"C:\nanofile\config.toml"));
        assert!(described.contains(r"C:\nanofile\config.toml"));
        assert!(described.contains("without a user login"));
    }

    #[test]
    fn an_image_path_is_compared_ignoring_case_and_quotes() {
        let expected = service_command_line(
            &PathBuf::from(r"C:\Apps\Nanofile\nanofile.exe"),
            &PathBuf::from(r"C:\Apps\Nanofile\config.toml"),
        );
        // What the registry holds after our own write.
        assert!(image_path_matches(&expected, &expected));
        // Case and a forward slash are the same path on Windows.
        let noisy = expected.to_lowercase().replace('\\', "/");
        assert!(image_path_matches(&noisy, &expected));
        // A service registered with only the executable still starts this
        // install, so it must not be reported as somebody else's.
        assert!(image_path_matches(
            r#""C:\Apps\Nanofile\nanofile.exe""#,
            &expected
        ));
    }

    #[test]
    fn another_install_is_not_ours() {
        let expected = service_command_line(
            &PathBuf::from(r"C:\Apps\Nanofile\nanofile.exe"),
            &PathBuf::from(r"C:\Apps\Nanofile\config.toml"),
        );
        // Same executable, different config file: it is not this installation's
        // service, and toggling the menu item has to say so.
        let other_config = service_command_line(
            &PathBuf::from(r"C:\Apps\Nanofile\nanofile.exe"),
            &PathBuf::from(r"D:\other\config.toml"),
        );
        assert!(!image_path_matches(&other_config, &expected));
        // A completely different binary.
        assert!(!image_path_matches(
            r#""C:\Elsewhere\nanofile.exe" --config "x""#,
            &expected
        ));
        assert!(!image_path_matches("", &expected));
    }

    #[test]
    fn the_executable_is_read_out_of_an_image_path() {
        assert_eq!(
            executable_in(r#""C:\Apps\Nanofile\nanofile.exe" service run --config "x""#),
            Some(r"C:\Apps\Nanofile\nanofile.exe".to_string())
        );
        // Unquoted (what the SCM writes for a path without spaces).
        assert_eq!(
            executable_in(r"C:\nanofile.exe service run"),
            Some(r"C:\nanofile.exe".to_string())
        );
        assert_eq!(executable_in(""), None);
        assert_eq!(executable_in("   "), None);
        assert_eq!(executable_in(r#"""#), None);
    }

    #[test]
    fn a_probe_knows_whether_it_is_ours() {
        assert!(ServiceProbe::Ours { running: true }.is_ours());
        assert!(ServiceProbe::Ours { running: true }.is_running());
        assert!(!ServiceProbe::Ours { running: false }.is_running());
        assert!(ServiceProbe::Ours { running: false }.is_installed());
        let other = ServiceProbe::OtherInstall {
            image_path: "x".into(),
            running: false,
            missing: true,
        };
        assert!(!other.is_ours());
        assert!(other.is_installed());
        assert!(other.is_broken(), "the registered file is gone");
        assert!(!ServiceProbe::Ours { running: true }.is_broken());
        assert!(!ServiceProbe::NotInstalled.is_installed());
        assert!(!ServiceProbe::Unknown.is_installed());
    }
}
