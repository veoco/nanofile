//! Optional system tray integration, compiled in with `--features tray`.
//!
//! Tray APIs need an event loop, and on macOS the tray must be created on the
//! main thread — so in tray mode the platform event loop owns the main thread
//! while the tokio runtime runs on background workers (see `main.rs`). Quit is
//! forwarded to the async server task, which shuts down gracefully.
//!
//! If something already answers on the configured address, typically the Windows
//! service, the menu comes up in client mode without a server: a second server
//! would only fail to bind, and a tray with no server of its own is what keeps
//! the service switchable while it runs.

mod actions;
mod icon;
pub(crate) mod icon_gen;
mod notify;
/// macOS uses a template image and lets the system invert it, so the theme probe
/// only exists where the raster has to carry the colour itself.
#[cfg(not(target_os = "macos"))]
mod theme;

#[cfg(target_os = "windows")]
#[path = "backend_windows.rs"]
mod backend;
#[cfg(target_os = "macos")]
#[path = "backend_macos.rs"]
mod backend;
#[cfg(target_os = "linux")]
#[path = "backend_linux.rs"]
mod backend;
#[cfg(not(any(target_os = "windows", target_os = "macos", target_os = "linux")))]
#[path = "backend_fallback.rs"]
mod backend;

use std::net::{IpAddr, Ipv6Addr, SocketAddr};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::Duration;

use anyhow::Context;
use infra::config::Config;
use server::i18n::I18n;
use tokio::sync::mpsc::UnboundedSender;
use tray_icon::menu::{CheckMenuItem, Menu, MenuItem, PredefinedMenuItem};
use tray_icon::{TrayIcon, TrayIconBuilder};

use crate::TrayCommand;
use crate::startup::login::{LoginEntry as _, PlatformLogin};
use crate::startup::policy;
use actions::MenuState;

const ID_OPEN_WEB: &str = "nanofile.open-web";
const ID_AUTOSTART: &str = "nanofile.autostart";
#[cfg(target_os = "windows")]
const ID_SERVICE: &str = "nanofile.service";
const ID_OPEN_CONFIG: &str = "nanofile.open-config";
const ID_QUIT: &str = "nanofile.quit";

pub(crate) struct TrayContext {
    exe_path: PathBuf,
    /// Absolute path of the config file this instance was started with; the
    /// auto-start entries pass it via `--config` from another working directory.
    config_path: PathBuf,
    /// `site_url` with a trailing slash, opened by the "Open Web UI" action.
    web_url: String,
    /// What the service preflight checks, from this instance's settings; the menu
    /// runs it before asking for elevation so a hopeless service is refused.
    #[cfg(target_os = "windows")]
    preflight: crate::startup::service::PreflightInput,
}

/// Whether the tray can and should run for this process and platform. A pure
/// check taken before logging is initialized (the log target depends on it).
pub enum RunMode {
    Tray,
    Headless(&'static str),
}

pub fn run_mode(config: &Config) -> RunMode {
    if !config.server.tray {
        return RunMode::Headless(
            "Tray disabled via config (server.tray = false), running headless",
        );
    }
    // The same question the first-run setup asks before opening a browser.
    if !server::desktop::has_desktop_session() {
        return RunMode::Headless(
            "No desktop session (DISPLAY/WAYLAND_DISPLAY unset), running headless",
        );
    }
    #[cfg(not(any(target_os = "windows", target_os = "macos", target_os = "linux")))]
    {
        return RunMode::Headless(
            "System tray is not supported on this platform, running headless",
        );
    }
    RunMode::Tray
}

/// Translation table for every user-visible tray string, resolved once in [`run`]:
/// the menu is built once, so its language is fixed for the process lifetime.
pub(super) fn lang() -> &'static I18n {
    TRAY_I18N.get_or_init(|| I18n::get(None))
}

static TRAY_I18N: OnceLock<&'static I18n> = OnceLock::new();

/// Pick the tray language: an explicit `ui.tray_language` override wins, then the
/// OS locale, then the web UI's `default_language`, then English.
fn resolve_lang(config: &Config) -> &'static I18n {
    resolve_lang_with(config, sys_locale::get_locale().as_deref())
}

fn resolve_lang_with(config: &Config, locale: Option<&str>) -> &'static I18n {
    let forced = config.ui.tray_language.trim();
    if !forced.is_empty()
        && !forced.eq_ignore_ascii_case("auto")
        && let Some(tag) = I18n::normalize_lang(forced)
    {
        return I18n::get(Some(tag));
    }
    if let Some(locale) = locale {
        let base = locale.split(['-', '_']).next().unwrap_or("");
        match base.to_ascii_lowercase().as_str() {
            "zh" => return I18n::get(Some("zh")),
            "en" => return I18n::get(Some("en")),
            _ => {}
        }
    }
    I18n::get(Some(&config.ui.default_language))
}

/// Runs the server on a background tokio runtime and blocks the main thread in
/// the platform tray event loop; never returns on its own, since the server task
/// exits the process when it is done (clean shutdown, Ctrl+C or error).
///
/// `listener` is the socket a completed first-run setup already served its page
/// on. While it is held, the "is somebody already serving this?" probe must not
/// run: it would connect to *this* process's own setup listener and turn a fresh
/// install into a tray that brings up no server at all.
pub fn run(
    config: Config,
    env_keys: infra::config::EnvKeys,
    config_path: PathBuf,
    listener: Option<server::restart::BoundListener>,
) -> ! {
    let _ = TRAY_I18N.set(resolve_lang(&config));

    let exe_path = absolute(
        std::env::current_exe()
            .expect("failed to locate the running executable")
            .as_path(),
    );
    // The menu is built from the config file, before the database is read, so a
    // `site_url` an administrator saves at runtime reaches it at the next start.
    let config_path = absolute(&config_path);
    let ctx = TrayContext {
        exe_path,
        #[cfg(target_os = "windows")]
        preflight: crate::startup::service::PreflightInput::from_config(&config, &config_path),
        config_path,
        web_url: format!("{}/", config.server.site_url.trim_end_matches('/')),
    };

    // Is something already serving this address? Almost always another nanofile
    // (the Windows service, or an instance the user forgot about). Binding anyway
    // would end the process with no tray left to switch the service off from, so
    // the tray comes up in client mode; a held socket is the one exception.
    let client_mode = listener.is_none() && already_serving(&config);

    let rt = tokio::runtime::Runtime::new().expect("failed to create tokio runtime");
    let (quit_tx, quit_rx) = tokio::sync::mpsc::unbounded_channel::<TrayCommand>();

    if client_mode {
        tracing::info!(
            web_url = %ctx.web_url,
            "something already serves this address; the tray starts without a server"
        );
    } else {
        rt.spawn(async move {
            let result = crate::run_server_flow(config, env_keys, Some(quit_rx), listener).await;
            match result {
                Ok(()) => std::process::exit(0),
                Err(e) => {
                    tracing::error!("Server failed: {e:#}");
                    std::process::exit(1);
                }
            }
        });
    }

    tracing::info!("Starting system tray icon");
    backend::run(&ctx, quit_tx, client_mode);
}

/// Whether something already accepts connections on the configured address.
/// Deliberately bounded: a slow or unusual setup must not delay the menu long.
fn already_serving(config: &Config) -> bool {
    let addr = probe_addr(&config.server.addr, config.server.port);
    std::net::TcpStream::connect_timeout(&addr, Duration::from_millis(250)).is_ok()
}

/// The address to probe for "is a server already there": a wildcard bind address
/// (`0.0.0.0`, `::`) is not connectable, so it maps to the loopback of its own
/// family; a host name, not resolvable here without blocking, falls back to the
/// same loopback.
fn probe_addr(addr: &str, port: u16) -> SocketAddr {
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

/// Whether *this* installation's Windows service is registered, which is always
/// false elsewhere: there the login entry is the only automatic start there is.
#[cfg(target_os = "windows")]
fn service_registered(config_path: &Path) -> bool {
    crate::startup::service::probe_service(config_path).is_ours()
}

#[cfg(not(target_os = "windows"))]
fn service_registered(_config_path: &Path) -> bool {
    false
}

/// Probe both automatic-start mechanisms once, for [`policy::plan`]: every decision
/// goes through this and the plan, so the "alternatives" rule cannot be applied
/// differently in two places.
fn startup_state(config_path: &Path, login: &PlatformLogin) -> policy::StartupState {
    policy::StartupState {
        login: login.state(),
        service_ours: service_registered(config_path),
    }
}

/// Blocks the main thread forever, used when tray initialization fails after the
/// server is already running: the process keeps serving headless, without an icon.
pub(super) fn park_forever() -> ! {
    loop {
        std::thread::park();
    }
}

/// Builds the tray icon and menu. Fails when the desktop integration is broken
/// (e.g. `DISPLAY` pointing at a dead X server); callers then run headless instead
/// of taking the server down.
fn create_tray(
    ctx: &TrayContext,
    quit_tx: UnboundedSender<TrayCommand>,
    client_mode: bool,
) -> anyhow::Result<TrayIcon> {
    let autostart = PlatformLogin::new(ctx.exe_path.clone(), ctx.config_path.clone());

    // Both mechanisms are probed *before* anything is written, and the one policy
    // decides: on Windows they are alternatives, so a stale login entry must not
    // be repointed while the service has replaced it.
    let state = startup_state(&ctx.config_path, &autostart);
    let plan = policy::plan(state);

    if plan.repair_login {
        // A login entry records absolute paths, so a folder that was moved or
        // deleted leaves it silently launching something that is not there.
        tracing::warn!(
            exe = %ctx.exe_path.display(),
            "the start-at-login entry points at a path that no longer exists; repointing it at \
             this installation"
        );
        if !login_entry_write_confirmed() {
            tracing::info!("repointing the start-at-login entry was not confirmed");
        } else if let Err(e) = autostart.enable() {
            tracing::warn!("repointing the start-at-login entry failed: {e:#}");
        }
    } else if plan.retire_login {
        tracing::info!("removing the start-at-login entry: the Windows service has replaced it");
        match autostart.retire_ours() {
            Ok(_) => {}
            Err(e) => tracing::warn!("could not remove the start-at-login entry: {e:#}"),
        }
    }

    // Showing the pair as alternatives is what keeps the menu honest: while this
    // installation's service is registered the login item is disabled.
    let t = lang();
    let item_open_web = MenuItem::with_id(ID_OPEN_WEB, t.tr("tray.open_web"), true, None);
    let item_autostart = CheckMenuItem::with_id(
        ID_AUTOSTART,
        t.tr("tray.launch_at_login"),
        plan.login_enabled,
        plan.login_checked,
        None,
    );
    // Checked only when the registered service is *this* installation's; one
    // registered for another copy is shown unchecked.
    #[cfg(target_os = "windows")]
    let item_service = CheckMenuItem::with_id(
        ID_SERVICE,
        t.tr("tray.service_autostart"),
        true,
        plan.service_checked,
        None,
    );
    let item_open_config = MenuItem::with_id(ID_OPEN_CONFIG, t.tr("tray.open_config"), true, None);
    let item_quit = MenuItem::with_id(ID_QUIT, t.tr("tray.quit"), true, None);

    let menu = Menu::new();
    menu.append(&item_open_web)
        .context("failed to build tray menu")?;
    menu.append(&item_open_config)
        .context("failed to build tray menu")?;
    menu.append(&PredefinedMenuItem::separator())
        .context("failed to build tray menu")?;
    menu.append(&item_autostart)
        .context("failed to build tray menu")?;
    #[cfg(target_os = "windows")]
    menu.append(&item_service)
        .context("failed to build tray menu")?;
    menu.append(&PredefinedMenuItem::separator())
        .context("failed to build tray menu")?;
    menu.append(&item_quit)
        .context("failed to build tray menu")?;

    actions::install(MenuState {
        ctx: TrayContext {
            exe_path: ctx.exe_path.clone(),
            config_path: ctx.config_path.clone(),
            web_url: ctx.web_url.clone(),
            #[cfg(target_os = "windows")]
            preflight: ctx.preflight.clone(),
        },
        login: autostart,
        login_item: item_autostart,
        #[cfg(target_os = "windows")]
        service_item: item_service,
        client_mode,
        quit_tx,
    });

    let builder = TrayIconBuilder::new()
        .with_id("nanofile")
        .with_menu(Box::new(menu))
        .with_menu_on_left_click(false)
        .with_tooltip(if client_mode {
            // "Nanofile is running" would be a lie about *this* process: it
            // brought up no server.
            lang().tr("tray.client_mode_tooltip")
        } else {
            lang().tr("tray.tooltip")
        });

    // macOS menu-bar icons are template images: the system inverts the glyph for
    // the light/dark menu bar and the highlighted state, so the glyph is rendered
    // bare. `with_icon_templated` is the only supported way to hand it over
    // already marked since tray-icon 0.26; elsewhere the raster carries the tile
    // colour itself.
    #[cfg(target_os = "macos")]
    let builder = builder.with_icon_templated(icon::tray_icon());
    #[cfg(not(target_os = "macos"))]
    let builder = builder.with_icon(icon::tray_icon());

    builder.build().context("failed to create tray icon")
}

/// Whether writing the login entry may proceed.
///
/// An elevated process writes to the elevated account's `HKCU` hive — a
/// *different* account's when a standard user typed an administrator's
/// credentials — so that case is confirmed instead of silently registering
/// auto-start for somebody else. Login startup itself always runs unelevated
/// (the exe manifest is `asInvoker`).
fn login_entry_write_confirmed() -> bool {
    #[cfg(target_os = "windows")]
    {
        !crate::startup::win32::is_elevated() || confirm_elevated_registration()
    }
    #[cfg(not(target_os = "windows"))]
    {
        true
    }
}

#[cfg(target_os = "windows")]
fn confirm_elevated_registration() -> bool {
    use windows_sys::Win32::UI::WindowsAndMessaging::{MB_ICONWARNING, MB_OKCANCEL, MessageBoxW};

    let t = lang();
    let user = std::env::var("USERNAME")
        .unwrap_or_else(|_| t.tr("tray.elevated_user_fallback").to_string());
    let text = t.trf("tray.elevated_body", &[("user", user.as_str())]);
    let text = crate::startup::win32::wide(&text);
    let caption = crate::startup::win32::wide(t.tr("tray.notify_title"));
    let result = unsafe {
        MessageBoxW(
            std::ptr::null_mut(),
            text.as_ptr(),
            caption.as_ptr(),
            MB_ICONWARNING | MB_OKCANCEL,
        )
    };
    result == 1 // IDOK
}

fn open_config_file(config_path: &Path) {
    tracing::info!("Opening config file {}", config_path.display());
    #[cfg(target_os = "windows")]
    {
        // Reveal the file in Explorer. `raw_arg` keeps the `/select,"…"` quoting
        // intact; Explorer always runs unelevated, so this works elevated too.
        use std::os::windows::process::CommandExt;
        if let Err(e) = std::process::Command::new("explorer")
            .raw_arg(format!("/select,\"{}\"", config_path.display()))
            .status()
        {
            tracing::warn!("Failed to open Explorer: {e}");
        }
    }
    #[cfg(target_os = "macos")]
    {
        if let Err(e) = std::process::Command::new("open")
            .arg("-R")
            .arg(config_path)
            .status()
        {
            tracing::warn!("Failed to reveal config file in Finder: {e}");
        }
    }
    #[cfg(target_os = "linux")]
    {
        let opened = std::process::Command::new("xdg-open")
            .arg(config_path)
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
        if !opened {
            // No handler for the file itself (or xdg-open missing): show its directory.
            let dir = config_path.parent().unwrap_or(config_path);
            if let Err(e) = std::process::Command::new("xdg-open").arg(dir).status() {
                tracing::warn!("Failed to open config directory: {e}");
            }
        }
    }
}

/// Canonical absolute path with Windows `\\?\` verbatim prefixes stripped,
/// falling back to cwd-joining for not-yet-existing files.
fn absolute(path: &Path) -> PathBuf {
    let resolved = path.canonicalize().unwrap_or_else(|_| {
        if path.is_absolute() {
            path.to_path_buf()
        } else {
            std::env::current_dir().unwrap_or_default().join(path)
        }
    });
    #[cfg(windows)]
    {
        let s = resolved.to_string_lossy();
        if let Some(rest) = s.strip_prefix(r"\\?\UNC\") {
            PathBuf::from(format!(r"\\{rest}"))
        } else if let Some(rest) = s.strip_prefix(r"\\?\") {
            PathBuf::from(rest)
        } else {
            resolved
        }
    }
    #[cfg(not(windows))]
    resolved
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_lang_prefers_override_then_os_locale() {
        let mut config = Config::default();
        config.ui.tray_language = "zh".into();
        assert_eq!(resolve_lang_with(&config, Some("en_US.UTF-8")).lang, "zh");
        config.ui.tray_language = "en".into();
        assert_eq!(resolve_lang_with(&config, Some("zh_CN")).lang, "en");
        config.ui.tray_language = "auto".into();

        assert_eq!(resolve_lang_with(&config, Some("zh_CN.UTF-8")).lang, "zh");
        assert_eq!(resolve_lang_with(&config, Some("en_US.UTF-8")).lang, "en");

        config.ui.default_language = "zh".into();
        assert_eq!(resolve_lang_with(&config, Some("fr_FR.UTF-8")).lang, "zh");
        assert_eq!(resolve_lang_with(&config, None).lang, "zh");
    }
}
