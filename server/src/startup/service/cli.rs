//! `nanofile service …`: the command-line front-end for the SCM registration.
//!
//! The elevated helper the tray menu starts is this same subcommand, so its
//! exit code is the tray's only report of what happened, and everything it
//! prints is invisible in that case (a GUI-subsystem build has no console
//! attached to it). Failures are therefore also logged and, for a human who
//! ran it by hand, shown in a message box.

use std::path::Path;

use infra::config::{Config, EnvKeys};

use super::windows::{install, probe, report_failure, run_as_service, uninstall};
use super::{ServiceAction, ServiceProbe};
use crate::startup::login::{LoginEntry as _, PlatformLogin};

/// `nanofile service …` entry point.
pub(crate) fn run_cli(
    action: ServiceAction,
    config: &Config,
    config_path: &Path,
    env_keys: EnvKeys,
) -> anyhow::Result<()> {
    match action {
        ServiceAction::Run => run_as_service(config.clone(), env_keys),
        ServiceAction::Install => match install(config_path) {
            Ok(()) => {
                retire_login_entry(config_path);
                println!(
                    "Nanofile registered as a Windows service; it starts at the next system \
                     start, without a login."
                );
                Ok(())
            }
            Err(e) => {
                report_failure("Nanofile could not be registered as a Windows service", &e);
                Err(e)
            }
        },
        ServiceAction::Uninstall => match uninstall() {
            Ok(()) => {
                // Deliberately *not* re-creating the login entry: "remove the
                // service" means Nanofile no longer starts automatically, and
                // writing a startup registration the user did not ask for in
                // that command would be worse than starting from an honest
                // blank.
                println!(
                    "Nanofile is no longer registered as a Windows service, and no longer \
                     starts automatically: install it again, or enable \"Start at login\" \
                     from the tray."
                );
                Ok(())
            }
            Err(e) => {
                report_failure("The Nanofile service could not be removed", &e);
                Err(e)
            }
        },
        ServiceAction::Status => {
            let state = probe(config_path);
            match &state {
                ServiceProbe::NotInstalled => {
                    println!("not installed");
                    std::process::exit(1);
                }
                ServiceProbe::Ours { running } => println!(
                    "installed for this installation ({})",
                    if *running { "running" } else { "stopped" }
                ),
                ServiceProbe::OtherInstall {
                    image_path,
                    running,
                    missing,
                } => println!(
                    "installed for another installation: {image_path} ({}{})",
                    if *running { "running" } else { "stopped" },
                    if *missing {
                        "; the registered executable no longer exists"
                    } else {
                        ""
                    }
                ),
                ServiceProbe::Unknown => {
                    println!("state unknown (the service control manager could not be queried)");
                    std::process::exit(1);
                }
            }
            Ok(())
        }
    }
}

/// Remove this installation's login entry: the service has replaced it.
///
/// The tray does the same thing after a successful install; this is what makes
/// `nanofile service install` mean the same as the menu item. Deliberately
/// narrow — `retire_ours()` removes the entry only when it names this
/// executable — because the elevated helper this command may *be* can have a
/// different administrator's `HKCU` hive.
fn retire_login_entry(config_path: &Path) {
    let Ok(exe) = std::env::current_exe() else {
        return;
    };
    let login = PlatformLogin::new(exe, config_path.to_path_buf());
    match login.retire_ours() {
        Ok(true) => println!("The start-at-login entry was removed: the service replaces it."),
        Ok(false) => {}
        Err(e) => tracing::warn!("could not remove the start-at-login entry: {e:#}"),
    }
}
