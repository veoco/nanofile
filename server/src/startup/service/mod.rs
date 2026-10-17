//! Windows service support: running nanofile under the Service Control Manager
//! (SCM), and registering/removing that service. A service starts at boot with
//! no session, unlike the login entry (`HKCU\…\Run`).
//!
//! The command line, `ImagePath` comparison, account and preflight checks are
//! platform-independent (tested everywhere); SCM/registry code is
//! `#[cfg(windows)]`. The service registers itself but never starts or stops
//! itself, so switching to it takes effect at the next system start: a port
//! whose connections were just closed stays in `TIME_WAIT` for minutes.

// The neutral surface below is only *reachable* from a Windows tray build;
// elsewhere the tests exercise it. One attribute instead of a per-item
// `#[allow(dead_code)]` — and where it ships, the lint stays active.
#![cfg_attr(not(all(target_os = "windows", feature = "tray")), allow(dead_code))]

use std::path::Path;

use crate::startup::cmdline::{normalize_win_path, win_cmd_quote};

/// The SCM name of the service, fixed (not derived from the install path) so
/// `sc query Nanofile` is predictable and the tray can find what it manages.
pub(crate) const SERVICE_NAME: &str = "Nanofile";

/// What the service is called in `services.msc`.
pub(crate) const DISPLAY_NAME: &str = "Nanofile Sync Server";

// One exit-code vocabulary for both processes the tray reads: the elevated
// helper's exit status, and the service's status in `services.msc` / `sc
// query`. `EXIT_REFUSED` means "nothing was changed on purpose" — neither a
// failure to report nor a success.

/// Failed; the reason is in the log file and (for the helper) in a dialog.
#[cfg(target_os = "windows")]
pub(crate) const EXIT_FAILED: i32 = 1;
/// Refused before changing anything: the operator cancelled, or a preflight
/// check failed.
#[cfg(target_os = "windows")]
pub(crate) const EXIT_REFUSED: i32 = 2;

#[cfg(target_os = "windows")]
#[derive(clap::Subcommand, Debug)]
pub(crate) enum ServiceAction {
    /// Run as a service. Called by the Service Control Manager (the registered
    /// binary path), never by hand: without the SCM this exits immediately.
    Run,
    /// Register (or update) the auto-start service; needs administrator rights.
    Install {
        /// Account the service runs under: `virtual` (the default,
        /// `NT SERVICE\Nanofile`), `system` (`LocalSystem`), `network`
        /// (`NT AUTHORITY\NetworkService`), or a local/domain account name,
        /// which needs a password.
        #[arg(long, value_name = "ACCOUNT", default_value = "virtual")]
        account: String,
        /// Password for a named account; visible in shell history and `ps`, so
        /// prefer --password-stdin or the prompt.
        #[arg(long)]
        password: Option<String>,
        /// Read the named account's password from the first line of stdin.
        #[arg(long, default_value_t = false, conflicts_with = "password")]
        password_stdin: bool,
    },
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
    /// (another copy of nanofile, or another config file). `missing` means that
    /// executable no longer exists, as after a moved or renamed install.
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

/// The registered binary path: the same executable, told which config file to
/// use (the SCM starts in `C:\Windows\System32`, so relative paths mean nothing).
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
/// quoted token when quoted, otherwise everything up to the first space. Only
/// used to ask whether that file still exists, so a hand-edited value can only
/// make the answer imprecise, never destructive.
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
/// write. Not exact equality on purpose: a hand-registered or older service may
/// carry fewer arguments, so only the executable or config file must differ.
pub(crate) fn image_path_matches(registered: &str, expected: &str) -> bool {
    let registered = normalize_win_path(registered);
    let expected = normalize_win_path(expected);
    !registered.is_empty()
        && !expected.is_empty()
        && (registered == expected
            || registered.starts_with(&expected)
            || expected.starts_with(&registered))
}

#[cfg(any(target_os = "windows", test))]
mod account;
#[cfg(target_os = "windows")]
mod acl;
#[cfg(any(target_os = "windows", test))]
mod checks;
#[cfg(target_os = "windows")]
mod cli;
#[cfg(target_os = "windows")]
mod preflight;
#[cfg(target_os = "windows")]
mod windows;

#[cfg(all(target_os = "windows", feature = "tray"))]
pub(crate) use account::ServiceAccount;
#[cfg(all(target_os = "windows", feature = "tray"))]
pub(crate) use checks::Severity;
#[cfg(target_os = "windows")]
pub(crate) use cli::run_cli;
#[cfg(all(target_os = "windows", feature = "tray"))]
pub(crate) use preflight::{Input as PreflightInput, run as run_preflight};
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
