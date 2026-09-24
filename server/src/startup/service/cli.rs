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
                println!("Nanofile is no longer registered as a Windows service.");
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
