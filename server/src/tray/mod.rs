//! Optional system tray integration, compiled in with `--features tray`.
//!
//! Threading model: tray APIs need an event loop, and on macOS the tray must
//! be created on the main thread — so in tray mode the platform event loop
//! owns the main thread while the tokio runtime runs on background worker
//! threads (see `main.rs`). Menu actions are handled on the event-loop thread;
//! "Quit" is forwarded to the async server task over a std mpsc channel, and
//! the server performs its normal graceful shutdown before ending the process
//! (which also removes the tray icon).

pub(crate) mod autostart;
mod icon;
pub(crate) mod icon_gen;
mod notify;

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

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::sync::mpsc::Sender;

use anyhow::Context;
use infra::config::Config;
use server::i18n::I18n;
use tray_icon::menu::{CheckMenuItem, Menu, MenuEvent, MenuItem, PredefinedMenuItem};
use tray_icon::{TrayIcon, TrayIconBuilder};

use crate::TrayCommand;
use autostart::Autostart as _;

const ID_OPEN_WEB: &str = "nanofile.open-web";
const ID_AUTOSTART: &str = "nanofile.autostart";
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
pub fn run(config: Config, config_path: PathBuf) -> ! {
    let _ = TRAY_I18N.set(resolve_lang(&config));

    let exe_path = absolute(
        std::env::current_exe()
            .expect("failed to locate the running executable")
            .as_path(),
    );
    let ctx = TrayContext {
        exe_path,
        config_path: absolute(&config_path),
        web_url: format!("{}/", config.server.site_url.trim_end_matches('/')),
    };

    let rt = tokio::runtime::Runtime::new().expect("failed to create tokio runtime");
    let (quit_tx, quit_rx) = std::sync::mpsc::channel::<TrayCommand>();

    rt.spawn(async move {
        let result = crate::run_server_flow(config, Some(quit_rx)).await;
        match result {
            Ok(()) => std::process::exit(0),
            Err(e) => {
                tracing::error!("Server failed: {e:#}");
                std::process::exit(1);
            }
        }
    });

    tracing::info!("Starting system tray icon");
    backend::run(&ctx, quit_tx);
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
fn create_tray(ctx: &TrayContext, quit_tx: Sender<TrayCommand>) -> anyhow::Result<TrayIcon> {
    let autostart =
        autostart::PlatformAutostart::new(ctx.exe_path.clone(), ctx.config_path.clone());

    let t = lang();
    let item_open_web = MenuItem::with_id(ID_OPEN_WEB, t.tr("tray.open_web"), true, None);
    let item_autostart = CheckMenuItem::with_id(
        ID_AUTOSTART,
        t.tr("tray.launch_at_login"),
        true,
        autostart.is_enabled(),
        None,
    );
    let item_open_config = MenuItem::with_id(ID_OPEN_CONFIG, t.tr("tray.open_config"), true, None);
    let item_quit = MenuItem::with_id(ID_QUIT, t.tr("tray.quit"), true, None);

    let menu = Menu::new();
    menu.append(&item_open_web)
        .context("failed to build tray menu")?;
    menu.append(&PredefinedMenuItem::separator())
        .context("failed to build tray menu")?;
    menu.append(&item_autostart)
        .context("failed to build tray menu")?;
    menu.append(&item_open_config)
        .context("failed to build tray menu")?;
    menu.append(&PredefinedMenuItem::separator())
        .context("failed to build tray menu")?;
    menu.append(&item_quit)
        .context("failed to build tray menu")?;

    MENU_STATE.with(|slot| {
        *slot.borrow_mut() = Some(MenuState {
            ctx: TrayContext {
                exe_path: ctx.exe_path.clone(),
                config_path: ctx.config_path.clone(),
                web_url: ctx.web_url.clone(),
            },
            autostart,
            autostart_item: item_autostart,
            quit_tx,
        });
    });
    MenuEvent::set_event_handler(Some(on_menu_event));

    TrayIconBuilder::new()
        .with_id("nanofile")
        .with_menu(Box::new(menu))
        .with_menu_on_left_click(false)
        .with_tooltip(lang().tr("tray.tooltip"))
        .with_icon(icon::tray_icon())
        .build()
        .context("failed to create tray icon")
}

struct MenuState {
    ctx: TrayContext,
    autostart: autostart::PlatformAutostart,
    autostart_item: CheckMenuItem,
    quit_tx: Sender<TrayCommand>,
}

thread_local! {
    /// Menu items and OS handles are only valid on the event-loop thread;
    /// keeping them here lets the (Send-bounded) muda event handler reach the
    /// shared state without ever moving anything across threads — the handler
    /// is always invoked on the loop thread.
    static MENU_STATE: RefCell<Option<MenuState>> = const { RefCell::new(None) };
}

/// Menu action snapshot, taken in a short borrow of `MENU_STATE` and executed
/// outside of it — the handler must never hold the RefCell across work that
/// pumps messages (modal dialogs, spawned processes), or a re-entrant menu
/// event would panic on the live borrow.
enum MenuAction {
    OpenWeb(String),
    ToggleAutostart(autostart::PlatformAutostart, CheckMenuItem),
    OpenConfig(PathBuf),
    Quit(Sender<TrayCommand>),
}

fn on_menu_event(event: MenuEvent) {
    let id: &str = event.id.as_ref();

    // muda invokes this handler synchronously inside its WM_COMMAND dispatch
    // (Windows) while it holds a borrow of the clicked item itself. Snapshot
    // the action in a short borrow and act outside of it, so a re-entrant
    // menu event can never touch a live borrow.
    let action = MENU_STATE.with(|slot| {
        let slot = slot.borrow();
        let state = slot.as_ref()?;
        match id {
            ID_OPEN_WEB => Some(MenuAction::OpenWeb(state.ctx.web_url.clone())),
            ID_AUTOSTART => Some(MenuAction::ToggleAutostart(
                state.autostart.clone(),
                state.autostart_item.clone(),
            )),
            ID_OPEN_CONFIG => Some(MenuAction::OpenConfig(state.ctx.config_path.clone())),
            ID_QUIT => Some(MenuAction::Quit(state.quit_tx.clone())),
            _ => None,
        }
    });

    match action {
        Some(MenuAction::OpenWeb(web_url)) => {
            tracing::info!("Opening web UI {web_url}");
            if let Err(e) = open::that(&web_url) {
                tracing::warn!("Failed to open web UI: {e}");
            }
        }
        Some(MenuAction::ToggleAutostart(autostart, item)) => {
            toggle_autostart(autostart, item);
        }
        Some(MenuAction::OpenConfig(config_path)) => open_config_file(&config_path),
        Some(MenuAction::Quit(quit_tx)) => {
            tracing::info!("Quit requested from tray");
            let _ = quit_tx.send(TrayCommand::Quit);
        }
        None => {}
    }
}

#[cfg(target_os = "windows")]
fn toggle_autostart(_autostart: autostart::PlatformAutostart, _item: CheckMenuItem) {
    // muda's WM_COMMAND dispatch is still on the stack: it holds a borrow of
    // the clicked item itself, so calling `set_checked` here panics — and the
    // registry toggle below may show a modal elevation dialog, which pumps
    // messages. Defer the whole toggle to the message loop via a thread
    // message; muda has already flipped the checkbox optimistically and the
    // deferred sync corrects it if the toggle fails or is cancelled.
    backend::request_autostart_toggle();
}

#[cfg(not(target_os = "windows"))]
fn toggle_autostart(autostart: autostart::PlatformAutostart, item: CheckMenuItem) {
    perform_autostart_toggle_with(autostart, item);
}

/// Run the launch-at-login toggle now. Only call outside muda's synchronous
/// menu dispatch (on Windows the backend loop invokes this from a posted
/// thread message), because it ends in a `set_checked` on the clicked item.
#[cfg(target_os = "windows")]
pub(super) fn perform_autostart_toggle() {
    let Some((autostart, item)) = MENU_STATE.with(|slot| {
        slot.borrow_mut()
            .as_ref()
            .map(|state| (state.autostart.clone(), state.autostart_item.clone()))
    }) else {
        return;
    };
    perform_autostart_toggle_with(autostart, item);
}

fn perform_autostart_toggle_with(autostart: autostart::PlatformAutostart, item: CheckMenuItem) {
    let result = if autostart.is_enabled() {
        tracing::info!("Disabling launch at login");
        autostart.disable()
    } else {
        tracing::info!("Enabling launch at login");
        autostart.enable()
    };
    if let Err(e) = result {
        tracing::error!("Failed to update launch-at-login: {e:#}");
    }
    item.set_checked(autostart.is_enabled());
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
