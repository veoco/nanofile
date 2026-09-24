//! Optional system tray integration, compiled in with `--features tray`.
//!
//! Threading model: tray APIs need an event loop, and on macOS the tray must
//! be created on the main thread — so in tray mode the platform event loop
//! owns the main thread while the tokio runtime runs on background worker
//! threads (see `main.rs`). Menu actions are handled on the event-loop thread;
//! "Quit" is forwarded to the async server task over a channel, and the server
//! performs its normal graceful shutdown before ending the process (which also
//! removes the tray icon).
//!
//! Two states this module can be in, decided before the menu is built:
//!
//! * **Server** — nothing is listening on the configured address, so this
//!   process starts the server and forwards Quit to it.
//! * **Client** — something already answers on that address, typically the
//!   Windows service. Starting a second server would only fail to bind, so the
//!   menu comes up without one; Quit then exits this process alone. That is what
//!   keeps the service switchable from the tray while the service is running.

mod actions;
mod icon;
pub(crate) mod icon_gen;
mod notify;
/// macOS gets a template image and lets the system invert it, so the theme
/// probe only exists where the raster has to carry the colour itself.
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
/// The Windows-only "start as a service" item.
#[cfg(target_os = "windows")]
const ID_SERVICE: &str = "nanofile.service";
const ID_OPEN_CONFIG: &str = "nanofile.open-config";
const ID_QUIT: &str = "nanofile.quit";

pub(crate) struct TrayContext {
    /// Absolute path of the running `nanofile` binary.
    exe_path: PathBuf,
    /// Absolute path of the config file this instance was started with — the
    /// auto-start entries pass it via `--config` so a login-started instance
    /// (whose working directory is not this one) finds the same config.
    config_path: PathBuf,
    /// `site_url` with a trailing slash, opened by the "Open Web UI" action.
    web_url: String,
}

/// Whether the tray can and should run for this process and platform.
///
/// A pure check, taken before logging is initialized (the log target depends
/// on it); callers log the outcome once the subscriber is up.
pub enum RunMode {
    /// Present the tray UI.
    Tray,
    /// Run headless, with the reason why.
    Headless(&'static str),
}

pub fn run_mode(config: &Config) -> RunMode {
    if !config.server.tray {
        return RunMode::Headless(
            "Tray disabled via config (server.tray = false), running headless",
        );
    }
    #[cfg(target_os = "linux")]
    {
        if std::env::var_os("DISPLAY").is_none() && std::env::var_os("WAYLAND_DISPLAY").is_none() {
            return RunMode::Headless(
                "No desktop session (DISPLAY/WAYLAND_DISPLAY unset), running headless",
            );
        }
    }
    #[cfg(not(any(target_os = "windows", target_os = "macos", target_os = "linux")))]
    {
        return RunMode::Headless(
            "System tray is not supported on this platform, running headless",
        );
    }
    RunMode::Tray
}

/// Translation table for every user-visible tray string (menu, tooltip,
/// notification, dialogs). Resolved once in [`run`] — the menu is built
/// once, so its language is fixed for the process lifetime.
pub(super) fn lang() -> &'static I18n {
    TRAY_I18N.get_or_init(|| I18n::get(None))
}

static TRAY_I18N: OnceLock<&'static I18n> = OnceLock::new();

/// Pick the tray language: an explicit `ui.tray_language` override wins,
/// then the OS locale (a Chinese system shows Chinese), then the web UI's
/// `default_language`, then English.
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
/// the platform tray event loop. Never returns on its own: the server task
/// exits the process when it is done (clean shutdown, Ctrl+C or error).
pub fn run(config: Config, env_keys: infra::config::EnvKeys, config_path: PathBuf) -> ! {
    let _ = TRAY_I18N.set(resolve_lang(&config));

    let exe_path = absolute(
        std::env::current_exe()
            .expect("failed to locate the running executable")
            .as_path(),
    );
    // The tray menu is built from the config file (it is opened before the
    // database is read). `site_url` is one of the settings an administrator can
    // change at runtime, so a saved value reaches the menu at the next start.
    let ctx = TrayContext {
        exe_path,
        config_path: absolute(&config_path),
        web_url: format!("{}/", config.server.site_url.trim_end_matches('/')),
    };

    // Is something already serving this address? Almost always another nanofile
    // (the Windows service, or an instance the user forgot about). Trying to
    // bind anyway would end the process with nothing but a log line, and there
    // would be no tray left to switch the service off from — so the tray comes
    // up in client mode instead.
    let client_mode = already_serving(&config);

    let rt = tokio::runtime::Runtime::new().expect("failed to create tokio runtime");
    let (quit_tx, quit_rx) = tokio::sync::mpsc::unbounded_channel::<TrayCommand>();

    if client_mode {
        tracing::info!(
            web_url = %ctx.web_url,
            "something already serves this address; the tray starts without a server"
        );
    } else {
        rt.spawn(async move {
            let result = crate::run_server_flow(config, env_keys, Some(quit_rx)).await;
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
///
/// Best effort, and deliberately bounded: this only decides whether the tray
/// should offer itself as a client, so a slow or unusual setup must not delay
/// the menu for long.
fn already_serving(config: &Config) -> bool {
    let addr = probe_addr(&config.server.addr, config.server.port);
    std::net::TcpStream::connect_timeout(&addr, Duration::from_millis(250)).is_ok()
}

/// The address to probe for "is a server already there".
///
/// A wildcard bind address (`0.0.0.0`, `::`) is not connectable, so it is mapped
/// to the loopback of its own family — which is where a second local instance
/// would be reachable anyway. A host name is not resolvable here without
/// blocking, so it falls back to the same loopback.
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

/// Whether *this* installation's Windows service is registered.
///
/// False everywhere else: on other platforms the login entry is the only
/// automatic start there is.
#[cfg(target_os = "windows")]
fn service_registered(config_path: &Path) -> bool {
    crate::startup::service::probe_service(config_path).is_ours()
}

#[cfg(not(target_os = "windows"))]
fn service_registered(_config_path: &Path) -> bool {
    false
}

/// Probe both automatic-start mechanisms once, for [`policy::plan`].
///
/// Every decision about them — the menu's initial state, the startup repair and
/// each toggle's re-synchronisation — goes through this and the plan, so the
/// "alternatives" rule cannot be applied differently in two places.
fn startup_state(config_path: &Path, login: &PlatformLogin) -> policy::StartupState {
    policy::StartupState {
        login: login.state(),
        service_ours: service_registered(config_path),
    }
}

/// Blocks the main thread forever. Used when tray initialization fails after
/// the server is already running in the background: the process keeps serving
/// headless, just without a tray icon.
pub(super) fn park_forever() -> ! {
    loop {
        std::thread::park();
    }
}

/// Builds the tray icon and menu. Fails when the desktop integration is
/// broken (e.g. `DISPLAY` points at a dead X server); callers fall back to
/// running headless instead of taking the server down.
fn create_tray(
    ctx: &TrayContext,
    quit_tx: UnboundedSender<TrayCommand>,
    client_mode: bool,
) -> anyhow::Result<TrayIcon> {
    let autostart = PlatformLogin::new(ctx.exe_path.clone(), ctx.config_path.clone());

    // Both mechanisms are probed *before* anything is written, and the one
    // policy decides what happens: on Windows they are alternatives, so a stale
    // login entry must not be repointed while the service has replaced it (that
    // would re-create the second automatic start the service removed), and an
    // entry of ours that survived an install is removed for the same reason.
    let state = startup_state(&ctx.config_path, &autostart);
    let plan = policy::plan(state);

    if plan.repair_login {
        // A login entry records absolute paths, so a folder that was moved,
        // renamed or deleted leaves it launching something that is not there —
        // and a login entry that fails does so silently. Point it at the copy
        // the user is actually running.
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
    // installation's service is registered the login item is disabled (removing
    // the service enables it again), instead of two checkmarks that contradict
    // each other and a click that does the opposite of what it looks like.
    let t = lang();
    let item_open_web = MenuItem::with_id(ID_OPEN_WEB, t.tr("tray.open_web"), true, None);
    let item_autostart = CheckMenuItem::with_id(
        ID_AUTOSTART,
        t.tr("tray.launch_at_login"),
        plan.login_enabled,
        plan.login_checked,
        None,
    );
    // Checked when the registered service is *this* installation's; a service
    // registered for another copy is shown unchecked (the confirmation names the
    // registered command line before replacing it).
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

    // Three groups: the two "open something" actions, the automatic-start pair
    // (the two that are alternatives to each other), and Quit.
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
        },
        login: autostart,
        login_item: item_autostart,
        #[cfg(target_os = "windows")]
        service_item: item_service,
        client_mode,
        quit_tx,
    });

    TrayIconBuilder::new()
        .with_id("nanofile")
        .with_menu(Box::new(menu))
        .with_menu_on_left_click(false)
        .with_tooltip(if client_mode {
            // Saying "Nanofile is running" here would be a lie about *this*
            // process: it brought up no server.
            lang().tr("tray.client_mode_tooltip")
        } else {
            lang().tr("tray.tooltip")
        })
        // macOS menu-bar icons are template images: the system inverts the
        // glyph for the light/dark menu bar and for the highlighted state, so
        // the glyph is rendered bare and ON TOP of that inversion. Elsewhere
        // the flag is ignored and the raster already carries the tile colour.
        .with_icon_as_template(cfg!(target_os = "macos"))
        .with_icon(icon::tray_icon())
        .build()
        .context("failed to create tray icon")
}

/// Whether writing the login entry may proceed.
///
/// An elevated process writes to the elevated account's `HKCU` hive, which is
/// the same hive when the user consented to elevation — but a *different*
/// account's when a standard user typed an administrator's credentials. The
/// consequence is made explicit instead of silently registering auto-start for
/// (potentially) somebody else. Login startup itself always runs unelevated
/// (the exe manifest is `asInvoker`), so no UAC prompt is involved here.
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
        // Reveal the file in Explorer. `raw_arg` keeps the `/select,"…"`
        // quoting intact (std would otherwise re-quote it); Explorer always
        // runs unelevated, so this also works from an elevated process.
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
            // No handler for the file itself (or xdg-open missing) — show the
            // containing directory instead.
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
        // Explicit override always wins.
        let mut config = Config::default();
        config.ui.tray_language = "zh".into();
        assert_eq!(resolve_lang_with(&config, Some("en_US.UTF-8")).lang, "zh");
        config.ui.tray_language = "en".into();
        assert_eq!(resolve_lang_with(&config, Some("zh_CN")).lang, "en");
        config.ui.tray_language = "auto".into();

        // "auto": the OS locale decides (a Chinese system shows Chinese).
        assert_eq!(resolve_lang_with(&config, Some("zh_CN.UTF-8")).lang, "zh");
        assert_eq!(resolve_lang_with(&config, Some("en_US.UTF-8")).lang, "en");

        // Unsupported locale falls back to the web UI's default language.
        config.ui.default_language = "zh".into();
        assert_eq!(resolve_lang_with(&config, Some("fr_FR.UTF-8")).lang, "zh");
        // No locale at all: same fallback chain.
        assert_eq!(resolve_lang_with(&config, None).lang, "zh");
    }
}
