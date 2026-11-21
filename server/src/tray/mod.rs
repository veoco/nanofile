//! Optional system tray integration, compiled in with `--features tray`.
//!
//! Each platform brings its own loop: Windows runs a Win32 message loop and macOS
//! hands the main thread to `NSApplication` (the tray must be created there), while
//! Linux' tray-icon backend serves the icon from a D-Bus thread and the main thread
//! only owns the menu and runs the actions queued for it. In every case the tokio
//! runtime runs on background workers (see `main.rs`), and Quit is forwarded to the
//! async server task, which shuts down gracefully.
//!
//! Whether this process serves at all is decided before the tray exists (see
//! `crate::startup::role`): while this installation's Windows service is registered
//! the tray is a client, so it never binds the address the service owns.

// `unsafe` is the Windows confirmation dialog; see `confirm_elevated_registration`.
#![cfg_attr(target_os = "windows", allow(unsafe_code))]

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

use std::cell::{Cell, RefCell};
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::OnceLock;

use anyhow::Context;
use infra::config::Config;
use server::i18n::I18n;
use tokio::sync::mpsc::UnboundedSender;
use tray_icon::menu::{CheckMenuItem, Menu, MenuItem, PredefinedMenuItem};
use tray_icon::{TrayIcon, TrayIconBuilder};

use crate::TrayCommand;
use crate::startup::login::LoginEntry as _;
use crate::startup::login::PlatformLogin;
use crate::startup::policy::{self, ServerStatus, StartupState};
use crate::startup::role;
use actions::MenuState;

const ID_OPEN_WEB: &str = "nanofile.open-web";
const ID_AUTOSTART: &str = "nanofile.autostart";
#[cfg(target_os = "windows")]
const ID_SERVICE: &str = "nanofile.service";
const ID_SERVICE_ACTION: &str = "nanofile.service-action";
const ID_OPEN_CONFIG: &str = "nanofile.open-config";
const ID_QUIT: &str = "nanofile.quit";

#[derive(Clone)]
pub(crate) struct TrayContext {
    exe_path: PathBuf,
    /// Absolute path of the config file this instance was started with; the
    /// auto-start entries pass it via `--config` from another working directory.
    config_path: PathBuf,
    /// `site_url` with a trailing slash, opened by the "Open Web UI" action.
    web_url: String,
    /// The address probed to tell "running" from "stopped" in the status row.
    probe: SocketAddr,
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

thread_local! {
    /// The tray icon itself, so a menu update can also refresh the tooltip.
    static TRAY: RefCell<Option<Rc<TrayIcon>>> = const { RefCell::new(None) };
    /// The last state `sync` rendered, for the one-shot startup notification.
    static STATUS: Cell<Option<ServerStatus>> = const { Cell::new(None) };
    /// The tooltip currently set, so the periodic refresh does not rewrite it.
    static LAST_TOOLTIP: RefCell<Option<String>> = const { RefCell::new(None) };
}

fn set_tooltip(text: &str) {
    // Before `builder.build()` there is no icon yet; do not record the text as set.
    let Some(tray) = TRAY.with(|slot| slot.borrow().clone()) else {
        return;
    };
    let changed = LAST_TOOLTIP.with(|slot| {
        let mut slot = slot.borrow_mut();
        if slot.as_deref() == Some(text) {
            false
        } else {
            *slot = Some(text.to_string());
            true
        }
    });
    if changed && let Err(e) = tray.set_tooltip(Some(text)) {
        tracing::debug!("Failed to update the tray tooltip: {e}");
    }
}

fn remember_status(status: ServerStatus) {
    STATUS.with(|cell| cell.set(Some(status)));
}

/// The status `sync` last rendered; used for the startup notification, which macOS
/// does not post.
#[cfg(not(target_os = "macos"))]
pub(super) fn status() -> Option<ServerStatus> {
    STATUS.with(Cell::get)
}

/// Pick the tray language: an explicit `ui.tray_language` override wins, then the
/// OS locale, then the web UI's `default_language`, then English.
fn resolve_lang(config: &Config) -> &'static I18n {
    resolve_lang_with(config, sys_locale::get_locale().as_deref())
}

/// The language the tray (and a headless tray build's fatal-error dialog) uses for
/// this config, resolved without initializing the tray icon.
pub(crate) fn language(config: &Config) -> &'static I18n {
    resolve_lang(config)
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
/// `serving` was decided in `main` (`startup::role`): `false` means this tray is
/// only a UI, either because this installation's Windows service is registered or
/// because something else already serves the address. `listener` is the socket a
/// completed first-run setup already served its page on; it implies `serving`.
pub fn run(
    config: Config,
    env_keys: infra::config::EnvKeys,
    config_path: PathBuf,
    listener: Option<server::restart::BoundListener>,
    serving: bool,
) -> ! {
    let _ = TRAY_I18N.set(language(&config));

    let exe_path = crate::startup::absolute_path(
        std::env::current_exe()
            .expect("failed to locate the running executable")
            .as_path(),
    );
    // The menu is built from the config file, before the database is read, so a
    // `site_url` an administrator saves at runtime reaches it at the next start.
    let config_path = crate::startup::absolute_path(&config_path);
    let ctx = TrayContext {
        exe_path,
        #[cfg(target_os = "windows")]
        preflight: crate::startup::service::PreflightInput::from_config(&config, &config_path),
        config_path,
        web_url: format!("{}/", config.server.site_url.trim_end_matches('/')),
        probe: role::probe_addr(&config.server.addr, config.server.port),
    };

    let rt = tokio::runtime::Runtime::new().expect("failed to create tokio runtime");
    let (quit_tx, quit_rx) = tokio::sync::mpsc::unbounded_channel::<TrayCommand>();

    let client_mode = !serving;
    if client_mode {
        tracing::info!(
            web_url = %ctx.web_url,
            "the tray starts without a server of its own"
        );
    } else {
        // Supervised: the server is a task like any other, and a task that panics
        // would otherwise vanish into a `JoinHandle` nobody reads — the tray icon
        // would stay up over a server that accepts nothing, and nothing would say so.
        // Waiting on it here means every way it can end (clean shutdown, error,
        // panic) leaves the process with a logged reason instead of a half-dead tray.
        let server = rt.spawn(async move {
            crate::run_server_flow(config, env_keys, Some(quit_rx), listener).await
        });
        rt.spawn(async move {
            match server.await {
                Ok(Ok(())) => std::process::exit(0),
                Ok(Err(e)) => {
                    tracing::error!("Server failed: {e:#}");
                    report_server_failure(&format!("{e:#}"));
                    std::process::exit(1);
                }
                Err(join) => {
                    tracing::error!("Server task ended without a shutdown signal: {join}");
                    report_server_failure(&join.to_string());
                    std::process::exit(1);
                }
            }
        });
    }

    tracing::info!("Starting system tray icon");
    backend::run(&ctx, quit_tx, client_mode);
}

/// A fatal server exit in a desktop build: without this the tray icon would simply
/// disappear, with the reason only in the log file.
fn report_server_failure(detail: &str) {
    let t = lang();
    let detail = crate::logging::dialog_text(detail, 2000);
    let text = t.trf("tray.server_failed_body", &[("detail", detail.as_str())]);
    crate::logging::show_message_box(t.tr("tray.notify_title"), &text);
}

/// The snapshot the menu is rendered from: both registrations, this process'
/// role, and whether something accepts connections on the address.
fn startup_state(ctx: &TrayContext, login: &PlatformLogin, serving: bool) -> StartupState {
    StartupState {
        service: role::service(&ctx.config_path),
        login: login.state(),
        port_busy: role::address_served(ctx.probe),
        self_serving: serving,
    }
}

/// Blocks the main thread forever, used when tray initialization fails while this
/// process is the server: it keeps serving headless, without an icon.
pub(super) fn park_forever() -> ! {
    loop {
        std::thread::park();
    }
}

/// A tray that could not be created: explain it where a desktop exists, then keep
/// the server (if this process is it) or end a client tray that has nothing to show.
pub(super) fn tray_failed(client_mode: bool, error: &anyhow::Error) -> ! {
    let t = lang();
    let detail = crate::logging::dialog_text(&format!("{error:#}"), 2000);
    let text = t.trf("tray.tray_unavailable", &[("detail", detail.as_str())]);
    crate::logging::show_message_box(t.tr("tray.notify_title"), &text);
    if client_mode {
        std::process::exit(1);
    }
    park_forever()
}

/// Builds the tray icon and menu. Fails when the desktop integration is broken
/// (e.g. `DISPLAY` pointing at a dead X server); callers then run headless instead
/// of taking the server down.
fn create_tray(
    ctx: &TrayContext,
    quit_tx: UnboundedSender<TrayCommand>,
    client_mode: bool,
) -> anyhow::Result<Rc<TrayIcon>> {
    let serving = !client_mode;
    let autostart = PlatformLogin::new(ctx.exe_path.clone(), ctx.config_path.clone());

    // Bind the login tray to the service before the menu is shown: start at boot
    // implies the tray at login. Only writes that the user asked for (or that the
    // binding requires) happen here; see `policy::plan`.
    let state = startup_state(ctx, &autostart, serving);
    let plan = policy::plan(&state);
    if plan.repair_login {
        // A login entry records absolute paths, so a folder that was moved or
        // deleted leaves it silently launching something that is not there. The
        // repair keeps Windows' own startup switch as it is: a user who turned the
        // entry off in Task Manager did not ask for it back.
        tracing::warn!(
            exe = %ctx.exe_path.display(),
            "the start-at-login entry points at a path that no longer exists; repointing it at \
             this installation"
        );
        if !login_entry_write_confirmed() {
            tracing::info!("repointing the start-at-login entry was not confirmed");
        } else if let Err(e) = autostart.repoint() {
            tracing::warn!("repointing the start-at-login entry failed: {e:#}");
        }
    } else if plan.ensure_login {
        tracing::info!("the Windows service is registered; adding the login tray entry");
        if !login_entry_write_confirmed() {
            tracing::info!("registering the login tray entry was not confirmed");
        } else if let Err(e) = autostart.enable() {
            tracing::warn!("registering the login tray entry failed: {e:#}");
        }
    }

    let t = lang();
    let item_open_web = MenuItem::with_id(ID_OPEN_WEB, t.tr("tray.open_web"), true, None);
    let item_open_config = MenuItem::with_id(ID_OPEN_CONFIG, t.tr("tray.open_config"), true, None);
    let item_autostart = CheckMenuItem::with_id(
        ID_AUTOSTART,
        t.tr("tray.launch_at_login"),
        true,
        plan.login_checked,
        None,
    );
    #[cfg(target_os = "windows")]
    let item_service = CheckMenuItem::with_id(
        ID_SERVICE,
        t.tr("tray.service_autostart"),
        true,
        plan.service_checked,
        None,
    );
    let item_status = MenuItem::new("", false, None);
    let item_action = MenuItem::with_id(ID_SERVICE_ACTION, "", true, None);
    let item_warning = MenuItem::new("", false, None);
    // In client mode the item only closes the UI; say so, so it does not suggest
    // that the service stops with it. In serving mode Quit does stop the server.
    let quit_key = if serving {
        "tray.quit"
    } else {
        "tray.close_tray"
    };
    let item_quit = MenuItem::with_id(ID_QUIT, t.tr(quit_key), true, None);

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
    menu.append(&item_status)
        .context("failed to build tray menu")?;
    menu.append(&PredefinedMenuItem::separator())
        .context("failed to build tray menu")?;
    menu.append(&item_quit)
        .context("failed to build tray menu")?;

    actions::install(MenuState {
        ctx: ctx.clone(),
        login: autostart,
        login_item: item_autostart,
        #[cfg(target_os = "windows")]
        service_item: item_service,
        open_web_item: item_open_web,
        status_item: item_status,
        action_item: item_action,
        warning_item: item_warning,
        menu: menu.clone(),
        action_attached: Cell::new(false),
        warning_attached: Cell::new(false),
        busy: Cell::new(false),
        #[cfg(target_os = "windows")]
        login_write_consented: Cell::new(false),
        #[cfg(target_os = "windows")]
        service_error_file: RefCell::new(None),
        serving,
        quit_tx,
    });

    let builder = TrayIconBuilder::new()
        .with_id("nanofile")
        .with_menu(Box::new(menu))
        // Windows and macOS only; on Linux the panel decides which button opens
        // the menu, and the SNI host does not forward that choice to the item.
        .with_menu_on_left_click(false)
        .with_tooltip(t.tr("tray.tooltip"));

    // Render the live snapshot before the menu is attached: a backend that mirrors
    // the menu (the Linux StatusNotifierItem) may not pick up later text changes.
    actions::sync();

    // macOS menu-bar icons are template images: the system inverts the glyph for
    // the light/dark menu bar and the highlighted state, so the glyph is rendered
    // bare. `with_icon_templated` is the only supported way to hand it over
    // already marked since tray-icon 0.26; elsewhere the raster carries the tile
    // colour itself.
    #[cfg(target_os = "macos")]
    let builder = builder.with_icon_templated(icon::tray_icon());
    #[cfg(not(target_os = "macos"))]
    let builder = builder.with_icon(icon::tray_icon());

    let tray = Rc::new(builder.build().context("failed to create tray icon")?);
    TRAY.with(|slot| *slot.borrow_mut() = Some(Rc::clone(&tray)));

    // Render the live snapshot over the placeholders above.
    actions::sync();

    // macOS has no timer hook on the NSApplication run loop, so refresh on the
    // interaction that precedes opening the menu. Best effort: if the menu is
    // already built by the time this runs, the next open has the fresh status.
    #[cfg(target_os = "macos")]
    {
        use tray_icon::{MouseButton, MouseButtonState, TrayIconEvent};
        TrayIconEvent::set_event_handler(Some(|event| {
            if matches!(
                event,
                TrayIconEvent::Click {
                    button: MouseButton::Right,
                    button_state: MouseButtonState::Up,
                    ..
                }
            ) {
                actions::sync();
            }
        }));
    }

    Ok(tray)
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
