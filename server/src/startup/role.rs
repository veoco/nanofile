//! Which role a `server` start takes.
//!
//! The decision is made before anything binds, and it is not a guess about the
//! port: while this installation's Windows service is registered, no `server`
//! start binds at all. The service owns the address; a second listener would
//! either fail or, worse, win the race at login and keep the service from starting
//! at the next boot. The port probe stays as the fallback for "something else is
//! already there".

use std::net::{IpAddr, Ipv6Addr, SocketAddr};
use std::path::Path;
use std::time::Duration;

use infra::config::Config;

use super::service::ServiceProbe;

/// What this process does with the configured address.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Role {
    /// The server runs here, and the tray is its desktop UI.
    ServeAndTray,
    /// The tray is only a UI: this installation's service (or something else)
    /// serves the address.
    TrayClient,
    /// The server runs here without a tray.
    ServeHeadless,
    /// This start must not bind: this installation's service is registered.
    Refuse,
}

impl Role {
    /// The `role` field of the startup log line.
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::ServeAndTray => "serve+tray",
            Self::TrayClient => "tray-client",
            Self::ServeHeadless => "serve-headless",
            Self::Refuse => "refuse",
        }
    }
}

/// This installation's Windows service; `NotInstalled` where there is no SCM.
pub(crate) fn service(config_path: &Path) -> ServiceProbe {
    super::service::probe_service(config_path)
}

/// Whether something already accepts connections on the configured address.
pub(crate) fn port_busy(config: &Config) -> bool {
    address_served(probe_addr(&config.server.addr, config.server.port))
}

/// Whether something accepts connections on `addr`. Deliberately bounded: a slow
/// or unusual peer must not delay the menu long.
pub(crate) fn address_served(addr: SocketAddr) -> bool {
    std::net::TcpStream::connect_timeout(&addr, Duration::from_millis(250)).is_ok()
}

/// The address to probe for "is a server already there": a wildcard bind address
/// (`0.0.0.0`, `::`) is not connectable, so it maps to the loopback of its own
/// family; a host name, not resolvable here without blocking, falls back to the
/// same loopback.
pub(crate) fn probe_addr(addr: &str, port: u16) -> SocketAddr {
    let host = addr.trim().trim_start_matches('[').trim_end_matches(']');
    match host.parse::<IpAddr>() {
        Ok(ip) if ip.is_unspecified() => match ip {
            IpAddr::V4(_) => SocketAddr::from(([127, 0, 0, 1], port)),
            IpAddr::V6(_) => SocketAddr::new(IpAddr::V6(Ipv6Addr::LOCALHOST), port),
        },
        Ok(ip) => SocketAddr::new(ip, port),
        Err(_) => SocketAddr::from(([127, 0, 0, 1], port)),
    }
}

/// Decide what this start does.
///
/// `tray_mode` already means "the tray build and a desktop session". `port_busy`
/// is the caller's probe of the configured address; a held setup listener is a
/// server of ours and must not be counted.
pub(crate) fn decide(
    tray_mode: bool,
    service: &ServiceProbe,
    port_busy: bool,
    allow_session_server: bool,
) -> Role {
    if matches!(service, ServiceProbe::Ours { .. }) && !allow_session_server {
        return if tray_mode {
            Role::TrayClient
        } else {
            Role::Refuse
        };
    }
    if tray_mode {
        if port_busy {
            Role::TrayClient
        } else {
            Role::ServeAndTray
        }
    } else {
        Role::ServeHeadless
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn a_registered_service_keeps_every_server_start_off_the_port() {
        for running in [true, false] {
            assert_eq!(
                decide(true, &ours(running), false, false),
                Role::TrayClient,
                "the tray is a client while the service is registered"
            );
            assert_eq!(
                decide(false, &ours(running), false, false),
                Role::Refuse,
                "a headless start would fight the service"
            );
        }
    }

    #[test]
    fn without_a_service_the_port_decides() {
        assert_eq!(
            decide(true, &ServiceProbe::NotInstalled, false, false),
            Role::ServeAndTray
        );
        assert_eq!(
            decide(true, &ServiceProbe::NotInstalled, true, false),
            Role::TrayClient
        );
        assert_eq!(
            decide(false, &ServiceProbe::NotInstalled, true, false),
            Role::ServeHeadless,
            "a headless start reports its own bind error"
        );
    }

    #[test]
    fn another_installations_service_is_only_a_port_probe() {
        assert_eq!(decide(true, &other(true), true, false), Role::TrayClient);
        assert_eq!(
            decide(true, &other(false), false, false),
            Role::ServeAndTray,
            "a stopped service of another copy is not this installation's"
        );
    }

    #[test]
    fn an_unknown_service_state_falls_back_to_the_port_probe() {
        assert_eq!(
            decide(true, &ServiceProbe::Unknown, false, false),
            Role::ServeAndTray
        );
        assert_eq!(
            decide(true, &ServiceProbe::Unknown, true, false),
            Role::TrayClient
        );
    }

    #[test]
    fn the_escape_hatch_restores_a_session_server() {
        assert_eq!(
            decide(false, &ours(false), false, true),
            Role::ServeHeadless
        );
        assert_eq!(decide(true, &ours(false), false, true), Role::ServeAndTray);
        assert_eq!(
            decide(true, &ours(false), true, true),
            Role::TrayClient,
            "the port probe still applies"
        );
    }
}
