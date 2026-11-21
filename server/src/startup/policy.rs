//! What the tray menu shows about the two auto-start mechanisms and this moment.
//!
//! Start at boot **implies** the login tray: the login entry is written when the
//! service is installed and is shown checked (and disabled) while the service is
//! registered. The one rule that makes that safe lives in `crate::startup::role`:
//! while the service is registered, a `server` start never binds the address. This
//! module only turns a probed snapshot into menu state; it writes nothing.

use super::login::LoginState;
use super::service::ServiceProbe;

/// Which process serves the configured address right now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ServerStatus {
    /// This tray process runs the server.
    RunningSelf,
    /// This installation's Windows service runs it.
    RunningService,
    /// Another installation's Windows service runs it.
    RunningOtherService,
    /// Something else accepts connections on the address.
    RunningOther,
    /// This installation's service is registered but stopped.
    StoppedService,
    /// Another installation's service is registered but stopped.
    StoppedOtherService,
    /// Nothing serves the address.
    Stopped,
    /// The service control manager could not be asked.
    Unknown,
}

impl ServerStatus {
    /// The `tray.server_*` key for this state.
    pub(crate) fn key(self) -> &'static str {
        match self {
            Self::RunningSelf => "tray.server_running_self",
            Self::RunningService => "tray.server_running_service",
            Self::RunningOtherService => "tray.server_running_other_service",
            Self::RunningOther => "tray.server_running_other",
            Self::StoppedService => "tray.server_stopped_service",
            Self::StoppedOtherService => "tray.server_stopped_other_service",
            Self::Stopped => "tray.server_stopped",
            Self::Unknown => "tray.server_unknown",
        }
    }

    /// Whether "Open web UI" can reach something.
    pub(crate) fn running(self) -> bool {
        matches!(
            self,
            Self::RunningSelf
                | Self::RunningService
                | Self::RunningOtherService
                | Self::RunningOther
        )
    }

    /// The tooltip key for this state.
    pub(crate) fn tooltip_key(self) -> &'static str {
        match self {
            Self::RunningSelf => "tray.tooltip",
            Self::RunningService | Self::RunningOtherService | Self::RunningOther => {
                "tray.tooltip_running"
            }
            Self::Unknown => "tray.tooltip_unknown",
            _ => "tray.tooltip_stopped",
        }
    }
}

/// The one service command the menu offers for the current state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ServiceActionKind {
    Start,
    Stop,
}

impl ServiceActionKind {
    pub(crate) fn key(self) -> &'static str {
        match self {
            Self::Start => "tray.start_service",
            Self::Stop => "tray.stop_service",
        }
    }
}

/// A fact the two checkboxes cannot express, shown as one warning row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Warning {
    /// A readable login entry belongs to another installation.
    ForeignLogin,
    /// A login entry exists but cannot be parsed as an (executable, config) pair.
    UnreadableLogin,
    /// This installation's own login entry points at a path that is gone.
    StaleLogin,
    /// The service is registered but no login entry was written (ensure failed).
    MissingLogin,
    /// Windows' own startup switch turned the login entry off.
    BlockedLogin,
    /// Another installation's service is registered and running, while this
    /// process serves.
    OtherServiceRunning,
    /// Another installation's service is registered but stopped.
    OtherServiceStopped,
    /// The service control manager could not be asked.
    ServiceUnknown,
    /// The service was just installed; it takes effect at the next boot.
    ServiceNextBoot,
    /// Nothing serves and this menu cannot start it (restart Nanofile).
    ServerStopped,
}

impl Warning {
    pub(crate) fn key(self) -> &'static str {
        match self {
            Self::ForeignLogin => "tray.warn_foreign_login",
            Self::UnreadableLogin => "tray.warn_unreadable_login",
            Self::StaleLogin => "tray.warn_login_stale",
            Self::MissingLogin => "tray.warn_login_missing",
            Self::BlockedLogin => "tray.warn_login_blocked",
            Self::OtherServiceRunning => "tray.warn_other_service_running",
            Self::OtherServiceStopped => "tray.warn_other_service_stopped",
            Self::ServiceUnknown => "tray.warn_service_unknown",
            Self::ServiceNextBoot => "tray.warn_service_next_boot",
            Self::ServerStopped => "tray.warn_server_stopped",
        }
    }
}

/// Everything the menu needs to know about the machine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct StartupState {
    pub service: ServiceProbe,
    pub login: LoginState,
    /// Something accepts connections on the configured address.
    pub port_busy: bool,
    /// This process brought up the server (so `role` chose `ServeAndTray`).
    pub self_serving: bool,
}

/// What to write before showing the menu, and how to show it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct StartupPlan {
    /// Repoint this installation's stale login entry at the running copy.
    pub repair_login: bool,
    /// Write the login entry: the service is registered and the entry is missing.
    pub ensure_login: bool,
    pub login_checked: bool,
    pub login_enabled: bool,
    pub service_checked: bool,
    pub status: ServerStatus,
    /// Start or stop this installation's service; `None` when it is not registered
    /// or when this process is the server.
    pub action: Option<ServiceActionKind>,
    pub warnings: Vec<Warning>,
    pub web_enabled: bool,
}

fn ours(service: &ServiceProbe) -> bool {
    matches!(service, ServiceProbe::Ours { .. })
}

fn running(service: &ServiceProbe) -> bool {
    matches!(
        service,
        ServiceProbe::Ours { running: true } | ServiceProbe::OtherInstall { running: true, .. }
    )
}

fn status(state: &StartupState) -> ServerStatus {
    if state.self_serving {
        return ServerStatus::RunningSelf;
    }
    match &state.service {
        ServiceProbe::Ours { running: true } => ServerStatus::RunningService,
        ServiceProbe::OtherInstall { running: true, .. } => ServerStatus::RunningOtherService,
        _ if state.port_busy => ServerStatus::RunningOther,
        ServiceProbe::Ours { running: false } => ServerStatus::StoppedService,
        ServiceProbe::OtherInstall { running: false, .. } => ServerStatus::StoppedOtherService,
        ServiceProbe::Unknown => ServerStatus::Unknown,
        ServiceProbe::NotInstalled => ServerStatus::Stopped,
    }
}

/// Decide what the menu shows and which repairs precede it.
pub(crate) fn plan(state: &StartupState) -> StartupPlan {
    let ours = ours(&state.service);
    let running = running(&state.service);
    let status = status(state);

    // Checked means "Windows will start something at login"; a stale or switched-off
    // entry is shown unchecked so a click can repair or re-enable it.
    let login_checked = state.login.present && !state.login.stale && state.login.approved;

    let mut warnings = Vec::new();
    if state.login.present {
        if !state.login.readable {
            warnings.push(Warning::UnreadableLogin);
        } else if !state.login.ours {
            warnings.push(Warning::ForeignLogin);
        } else if state.login.stale {
            warnings.push(Warning::StaleLogin);
        } else if !state.login.approved {
            warnings.push(Warning::BlockedLogin);
        }
    } else if ours {
        warnings.push(Warning::MissingLogin);
    }
    match &state.service {
        ServiceProbe::OtherInstall { running, .. } if state.self_serving => {
            warnings.push(if *running {
                Warning::OtherServiceRunning
            } else {
                Warning::OtherServiceStopped
            });
        }
        ServiceProbe::Unknown => warnings.push(Warning::ServiceUnknown),
        ServiceProbe::Ours { .. } if state.self_serving => warnings.push(Warning::ServiceNextBoot),
        _ => {}
    }
    if matches!(
        status,
        ServerStatus::Stopped | ServerStatus::StoppedOtherService
    ) {
        warnings.push(Warning::ServerStopped);
    }

    StartupPlan {
        repair_login: state.login.ours && state.login.stale,
        ensure_login: ours && !state.login.present,
        login_checked,
        // Disabled while the bound entry really works. A missing, stale, blocked or
        // unreadable entry stays clickable so the menu can repair it.
        login_enabled: !ours || !login_checked || !state.login.readable,
        service_checked: ours,
        status,
        action: if ours && !state.self_serving {
            Some(if running {
                ServiceActionKind::Stop
            } else {
                ServiceActionKind::Start
            })
        } else {
            None
        },
        warnings,
        web_enabled: status.running(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn login(present: bool, readable: bool, ours: bool, stale: bool, approved: bool) -> LoginState {
        LoginState {
            present,
            readable,
            ours,
            stale,
            approved,
        }
    }

    fn healthy() -> LoginState {
        login(true, true, true, false, true)
    }

    fn state(service: ServiceProbe, login: LoginState) -> StartupState {
        StartupState {
            service,
            login,
            port_busy: false,
            self_serving: false,
        }
    }

    fn ours(running: bool) -> ServiceProbe {
        ServiceProbe::Ours { running }
    }

    fn other(running: bool) -> ServiceProbe {
        ServiceProbe::OtherInstall {
            image_path: r"C:\Apps\Nanofile\nanofile.exe".to_string(),
            running,
            missing: false,
        }
    }

    #[test]
    fn a_plain_session_server_shows_one_running_state_and_no_actions() {
        let plan = plan(&StartupState {
            service: ServiceProbe::NotInstalled,
            login: login(false, false, false, false, true),
            port_busy: false,
            self_serving: true,
        });
        assert_eq!(plan.status, ServerStatus::RunningSelf);
        assert!(plan.web_enabled);
        assert!(!plan.login_checked);
        assert!(plan.login_enabled);
        assert!(!plan.service_checked);
        assert!(plan.action.is_none());
        assert!(plan.warnings.is_empty());
        assert!(!plan.repair_login && !plan.ensure_login);
    }

    #[test]
    fn a_running_service_shows_the_tray_bound_and_offers_stop() {
        let plan = plan(&state(ours(true), healthy()));
        assert_eq!(plan.status, ServerStatus::RunningService);
        assert!(plan.service_checked);
        assert!(plan.login_checked);
        assert!(!plan.login_enabled, "the bound tray item is not clickable");
        assert_eq!(plan.action, Some(ServiceActionKind::Stop));
        assert!(plan.warnings.is_empty());
        assert!(plan.web_enabled);
    }

    #[test]
    fn a_stopped_service_offers_start_and_disables_the_web_ui() {
        let plan = plan(&state(ours(false), healthy()));
        assert_eq!(plan.status, ServerStatus::StoppedService);
        assert_eq!(plan.action, Some(ServiceActionKind::Start));
        assert!(!plan.web_enabled);
        assert!(!plan.login_enabled);
    }

    #[test]
    fn a_missing_login_entry_is_ensured_and_reported() {
        let plan = plan(&state(ours(true), login(false, false, false, false, true)));
        assert!(plan.ensure_login);
        assert_eq!(plan.warnings, vec![Warning::MissingLogin]);
        assert!(!plan.login_checked);
        assert!(plan.login_enabled, "a click writes the missing entry");
    }

    #[test]
    fn a_stale_own_entry_is_repaired_and_shown_unchecked() {
        let plan = plan(&state(ours(true), login(true, true, true, true, true)));
        assert!(plan.repair_login);
        assert!(!plan.login_checked);
        assert!(plan.login_enabled);
        assert_eq!(plan.warnings, vec![Warning::StaleLogin]);
    }

    #[test]
    fn a_switched_off_entry_is_unchecked_and_clickable() {
        let plan = plan(&state(ours(true), login(true, true, true, false, false)));
        assert!(!plan.login_checked);
        assert!(plan.login_enabled);
        assert_eq!(plan.warnings, vec![Warning::BlockedLogin]);
        assert!(!plan.repair_login && !plan.ensure_login);
    }

    #[test]
    fn an_unreadable_entry_is_reported_and_can_be_replaced() {
        let plan = plan(&state(ours(true), login(true, false, false, false, true)));
        assert!(plan.login_checked);
        assert!(plan.login_enabled, "a click removes what cannot be read");
        assert_eq!(plan.warnings, vec![Warning::UnreadableLogin]);
    }

    #[test]
    fn a_healthy_entry_of_another_installation_is_bound_but_warned() {
        let plan = plan(&state(ours(true), login(true, true, false, false, true)));
        assert!(plan.login_checked);
        assert!(!plan.login_enabled);
        assert_eq!(plan.warnings, vec![Warning::ForeignLogin]);
        assert!(!plan.repair_login, "a live foreign entry is not taken over");
    }

    #[test]
    fn a_service_installed_mid_session_is_announced() {
        let plan = plan(&StartupState {
            service: ours(false),
            login: healthy(),
            port_busy: false,
            self_serving: true,
        });
        assert_eq!(plan.status, ServerStatus::RunningSelf);
        assert!(
            plan.action.is_none(),
            "starting it now would fight this process"
        );
        assert_eq!(plan.warnings, vec![Warning::ServiceNextBoot]);
        assert!(plan.login_checked && !plan.login_enabled);
    }

    #[test]
    fn another_installations_service_is_named_in_both_run_states() {
        let running = plan(&StartupState {
            service: other(true),
            login: healthy(),
            port_busy: true,
            self_serving: false,
        });
        assert_eq!(running.status, ServerStatus::RunningOtherService);
        assert!(running.action.is_none());
        assert!(running.warnings.is_empty());

        let serving = plan(&StartupState {
            service: other(false),
            login: healthy(),
            port_busy: false,
            self_serving: true,
        });
        assert_eq!(serving.status, ServerStatus::RunningSelf);
        assert_eq!(serving.warnings, vec![Warning::OtherServiceStopped]);
    }

    #[test]
    fn an_unknown_service_falls_back_to_the_port() {
        let busy = plan(&StartupState {
            service: ServiceProbe::Unknown,
            login: healthy(),
            port_busy: true,
            self_serving: false,
        });
        assert_eq!(busy.status, ServerStatus::RunningOther);
        assert!(busy.web_enabled);
        assert_eq!(busy.warnings, vec![Warning::ServiceUnknown]);

        let idle = plan(&StartupState {
            service: ServiceProbe::Unknown,
            login: healthy(),
            port_busy: false,
            self_serving: false,
        });
        assert_eq!(idle.status, ServerStatus::Unknown);
        assert!(!idle.web_enabled);
        assert_eq!(idle.warnings, vec![Warning::ServiceUnknown]);
    }

    #[test]
    fn a_client_with_nothing_left_to_attach_to_says_so() {
        let plan = plan(&StartupState {
            service: ServiceProbe::NotInstalled,
            login: healthy(),
            port_busy: false,
            self_serving: false,
        });
        assert_eq!(plan.status, ServerStatus::Stopped);
        assert!(!plan.web_enabled);
        assert_eq!(plan.warnings, vec![Warning::ServerStopped]);
    }
}
