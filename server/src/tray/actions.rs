//! Everything the tray menu *does*.
//!
//! Shape forced by muda: while a menu item is activated it is still borrowed, and
//! on Windows the handler runs inside its `WM_COMMAND` dispatch — so it may not
//! touch that item and may not block on anything that pumps messages (the dialogs
//! do). It only queues a [`TrayAction`]; the item updates and dialogs happen when
//! the owning thread drains the queue:
//!
//! - Windows: the message loop drains it after the dispatch returns.
//! - Linux: tray-icon's D-Bus thread invokes the handler, so the action travels
//!   over the backend's wake channel to the main thread, which owns the menu.
//! - macOS: the `NSApplication` main loop runs the action directly.
//!
//! Queuing is also why a service operation needs no lock: a second click is
//! prevented by disabling the items while the helper runs.

// `unsafe` is the Win32 process/dialog calls; the re-entrancy contract is in the
// module doc above.
#![cfg_attr(target_os = "windows", allow(unsafe_code))]

use std::cell::{Cell, RefCell};
#[cfg(target_os = "windows")]
use std::collections::VecDeque;
#[cfg(target_os = "windows")]
use std::path::{Path, PathBuf};

use tokio::sync::mpsc::UnboundedSender;
use tray_icon::menu::{CheckMenuItem, Menu, MenuEvent, MenuItem};

use crate::TrayCommand;
use crate::startup::login::{LoginEntry as _, PlatformLogin};
use crate::startup::policy;
#[cfg(target_os = "windows")]
use crate::startup::service::ServiceProbe;

#[cfg(any(target_os = "windows", target_os = "linux"))]
use super::backend;
use super::open_config_file;
use super::{ID_AUTOSTART, ID_OPEN_CONFIG, ID_OPEN_WEB, ID_QUIT, TrayContext, lang};
#[cfg(target_os = "windows")]
use super::{ID_SERVICE, ID_SERVICE_ACTION};
#[cfg(target_os = "windows")]
use crate::startup::service::{EXIT_REFUSED, probe_service};
#[cfg(target_os = "windows")]
use windows_sys::Win32::Foundation::{
    CloseHandle, ERROR_CANCELLED, GetLastError, HANDLE, STILL_ACTIVE,
};
#[cfg(target_os = "windows")]
use windows_sys::Win32::System::Com::CoInitializeEx;
#[cfg(target_os = "windows")]
use windows_sys::Win32::System::Threading::{GetExitCodeProcess, WaitForSingleObject};
#[cfg(target_os = "windows")]
use windows_sys::Win32::UI::Shell::{
    SEE_MASK_NOASYNC, SEE_MASK_NOCLOSEPROCESS, SHELLEXECUTEINFOW, ShellExecuteExW,
};
#[cfg(target_os = "windows")]
use windows_sys::Win32::UI::WindowsAndMessaging::{
    IDOK, MB_ICONERROR, MB_ICONINFORMATION, MB_ICONQUESTION, MB_ICONWARNING, MB_OK, MB_OKCANCEL,
    MB_SETFOREGROUND, MB_TOPMOST, MessageBoxW, SW_HIDE,
};

/// Where the action row is inserted: right after the status row. The base menu is
/// open-web, open-config, sep, login, `service`, sep, status, sep, quit.
#[cfg(target_os = "windows")]
const ACTION_POSITION: usize = 7;
#[cfg(not(target_os = "windows"))]
const ACTION_POSITION: usize = 6;

/// Everything a menu action needs, plus the items it has to update. Kept in a
/// thread-local because menu items and OS handles are only valid on the event-loop
/// thread.
pub(super) struct MenuState {
    pub(super) ctx: TrayContext,
    pub(super) login: PlatformLogin,
    pub(super) login_item: CheckMenuItem,
    #[cfg(target_os = "windows")]
    pub(super) service_item: CheckMenuItem,
    pub(super) open_web_item: MenuItem,
    pub(super) status_item: MenuItem,
    pub(super) action_item: MenuItem,
    pub(super) warning_item: MenuItem,
    /// The menu itself, so the action and warning rows can come and go.
    pub(super) menu: Menu,
    pub(super) action_attached: Cell<bool>,
    pub(super) warning_attached: Cell<bool>,
    /// A service helper is running: both self-start items and the action wait.
    pub(super) busy: Cell<bool>,
    /// The install confirmation named the elevated account, so it is also the
    /// consent to write that account's login entry; no second dialog.
    #[cfg(target_os = "windows")]
    pub(super) login_write_consented: Cell<bool>,
    /// File next to the executable the elevated helper writes its failure reason
    /// to, so the tray can show one localized dialog with the detail.
    #[cfg(target_os = "windows")]
    pub(super) service_error_file: RefCell<Option<PathBuf>>,
    /// This process brought up the server (so Quit stops it).
    pub(super) serving: bool,
    pub(super) quit_tx: UnboundedSender<TrayCommand>,
}

thread_local! {
    static MENU_STATE: RefCell<Option<MenuState>> = const { RefCell::new(None) };
    #[cfg(target_os = "windows")]
    static PENDING: RefCell<VecDeque<TrayAction>> = const { RefCell::new(VecDeque::new()) };
}

pub(super) fn install(state: MenuState) {
    MENU_STATE.with(|slot| *slot.borrow_mut() = Some(state));
    MenuEvent::set_event_handler(Some(on_menu_event));
}

fn with_state<T>(f: impl FnOnce(&MenuState) -> T) -> Option<T> {
    MENU_STATE.with(|slot| slot.borrow().as_ref().map(f))
}

/// Re-probe the machine and render the menu from the result.
///
/// Takes the state out for the duration so nothing inside can re-borrow it; the
/// menu, the items and the tooltip are all owned by the event-loop thread.
pub(super) fn sync() {
    let state = MENU_STATE.with(|slot| slot.borrow_mut().take());
    if let Some(state) = state {
        sync_state(&state);
        MENU_STATE.with(|slot| *slot.borrow_mut() = Some(state));
    }
}

fn sync_state(state: &MenuState) {
    let snapshot = super::startup_state(&state.ctx, &state.login, state.serving);
    let plan = policy::plan(&snapshot);
    let t = lang();
    let busy = state.busy.get();

    set_enabled(&state.open_web_item, plan.web_enabled);
    set_check(
        &state.login_item,
        plan.login_checked,
        plan.login_enabled && !busy,
    );
    #[cfg(target_os = "windows")]
    set_check(&state.service_item, plan.service_checked, !busy);
    set_text(&state.status_item, t.tr(plan.status.key()));

    match plan.action {
        Some(kind) => {
            set_text(&state.action_item, t.tr(kind.key()));
            set_enabled(&state.action_item, !busy);
            if !state.action_attached.get() {
                let _ = state.menu.insert(&state.action_item, ACTION_POSITION);
                state.action_attached.set(true);
            }
        }
        None => {
            if state.action_attached.get() {
                let _ = state.menu.remove(&state.action_item);
                state.action_attached.set(false);
            }
        }
    }

    if plan.warnings.is_empty() {
        if state.warning_attached.get() {
            let _ = state.menu.remove(&state.warning_item);
            state.warning_attached.set(false);
        }
    } else {
        set_text(&state.warning_item, &join_warnings(&plan.warnings));
        if !state.warning_attached.get() {
            let position = ACTION_POSITION + usize::from(state.action_attached.get());
            let _ = state.menu.insert(&state.warning_item, position);
            state.warning_attached.set(true);
        }
    }

    super::set_tooltip(t.tr(plan.status.tooltip_key()));
    super::remember_status(plan.status);
}

/// Only touch muda items whose value actually changed: the periodic refresh must
/// not make the Linux menu watcher rebuild the menu every few seconds.
fn set_text(item: &MenuItem, text: &str) {
    if item.text().as_str() != text {
        item.set_text(text);
    }
}

fn set_enabled(item: &MenuItem, enabled: bool) {
    if item.is_enabled() != enabled {
        item.set_enabled(enabled);
    }
}

fn set_check(item: &CheckMenuItem, checked: bool, enabled: bool) {
    if item.is_checked() != checked {
        item.set_checked(checked);
    }
    if item.is_enabled() != enabled {
        item.set_enabled(enabled);
    }
}

fn join_warnings(warnings: &[policy::Warning]) -> String {
    let t = lang();
    let separator = if t.lang == "zh" { "；" } else { "; " };
    warnings
        .iter()
        .map(|warning| t.tr(warning.key()))
        .collect::<Vec<_>>()
        .join(separator)
}

/// What a menu click asks for: no payloads, since the action reads what it needs
/// from [`MENU_STATE`] when it runs, outside muda's borrow.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) enum TrayAction {
    OpenWeb,
    ToggleLogin,
    #[cfg(target_os = "windows")]
    ToggleService,
    #[cfg(target_os = "windows")]
    ServiceAction,
    OpenConfig,
    Quit,
}

fn on_menu_event(event: MenuEvent) {
    if let Some(action) = action_for(event.id.as_ref()) {
        defer(action);
    }
}

/// The id-to-action table, kept separate so the mapping can be tested without a
/// menu or an event handler.
fn action_for(id: &str) -> Option<TrayAction> {
    match id {
        ID_OPEN_WEB => Some(TrayAction::OpenWeb),
        ID_AUTOSTART => Some(TrayAction::ToggleLogin),
        #[cfg(target_os = "windows")]
        ID_SERVICE => Some(TrayAction::ToggleService),
        #[cfg(target_os = "windows")]
        ID_SERVICE_ACTION => Some(TrayAction::ServiceAction),
        ID_OPEN_CONFIG => Some(TrayAction::OpenConfig),
        ID_QUIT => Some(TrayAction::Quit),
        _ => None,
    }
}

fn defer(action: TrayAction) {
    #[cfg(target_os = "windows")]
    {
        PENDING.with(|queue| queue.borrow_mut().push_back(action));
        if !backend::post_action() {
            // Best effort: the loop should be running, but apply it now if not.
            tracing::warn!("Tray loop not running, applying the menu action directly");
            drain();
        }
    }
    #[cfg(target_os = "linux")]
    {
        // The handler runs on tray-icon's D-Bus thread; the menu is owned by the
        // main thread, which drains the action there.
        backend::defer(action);
    }
    #[cfg(target_os = "macos")]
    {
        action.run();
    }
    #[cfg(not(any(target_os = "windows", target_os = "linux", target_os = "macos")))]
    {
        // No tray is ever started on these platforms (`run_mode` reports headless).
        let _ = action;
    }
}

/// Run every queued action, called from the message loop after muda's dispatch.
#[cfg(target_os = "windows")]
pub(super) fn drain() {
    while let Some(action) = PENDING.with(|queue| queue.borrow_mut().pop_front()) {
        action.run();
    }
}

impl TrayAction {
    pub(super) fn run(self) {
        match self {
            TrayAction::OpenWeb => {
                let Some(web_url) = with_state(|state| state.ctx.web_url.clone()) else {
                    return;
                };
                tracing::info!("Opening web UI {web_url}");
                if let Err(e) = open::that(&web_url) {
                    tracing::warn!("Failed to open web UI: {e}");
                }
            }
            TrayAction::ToggleLogin => toggle_login(),
            #[cfg(target_os = "windows")]
            TrayAction::ToggleService => toggle_service(),
            #[cfg(target_os = "windows")]
            TrayAction::ServiceAction => service_action(),
            TrayAction::OpenConfig => {
                let Some(config_path) = with_state(|state| state.ctx.config_path.clone()) else {
                    return;
                };
                open_config_file(&config_path);
            }
            TrayAction::Quit => quit(),
        }
    }
}

fn toggle_login() {
    let Some(login) = with_state(|state| state.login.clone()) else {
        return;
    };
    perform_login_toggle(login);
}

fn perform_login_toggle(login: PlatformLogin) {
    // Registered *and* not switched off in Windows' own startup settings: a click
    // on an entry that Task Manager disabled turns it back on, which is what an
    // unchecked box means.
    let result = if login.starts_at_login() {
        tracing::info!("Disabling launch at login");
        login.disable()
    } else if !super::login_entry_write_confirmed() {
        // The elevated-registration warning was dismissed; render the state that
        // is actually there.
        tracing::info!("launch-at-login registration cancelled");
        Ok(())
    } else {
        tracing::info!("Enabling launch at login");
        login.enable()
    };
    if let Err(e) = result {
        tracing::error!("Failed to update launch-at-login: {e:#}");
        let detail = crate::logging::dialog_text(&format!("{e:#}"), 1000);
        super::notify::login_failed(&detail);
    }
    sync();
}

fn quit() {
    let Some((quit_tx, serving)) = with_state(|state| (state.quit_tx.clone(), state.serving))
    else {
        return;
    };
    if serving {
        tracing::info!("Quit requested from tray");
        let _ = quit_tx.send(TrayCommand::Quit);
    } else {
        // A client tray owns no server: Quit closes the tray, not the service.
        tracing::info!("Quit requested from a client-mode tray");
        std::process::exit(0);
    }
}

/// What one elevated helper was asked to do, carried through the wait thread.
#[cfg(target_os = "windows")]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ServiceOutcome {
    Installed,
    Uninstalled,
    Started,
    Stopped,
}

#[cfg(target_os = "windows")]
impl ServiceOutcome {
    pub(super) fn wparam(self) -> usize {
        match self {
            Self::Installed => 0,
            Self::Uninstalled => 1,
            Self::Started => 2,
            Self::Stopped => 3,
        }
    }

    pub(super) fn from_wparam(value: usize) -> Option<Self> {
        match value {
            0 => Some(Self::Installed),
            1 => Some(Self::Uninstalled),
            2 => Some(Self::Started),
            3 => Some(Self::Stopped),
            _ => None,
        }
    }

    fn verb(self) -> &'static str {
        match self {
            Self::Installed => "install",
            Self::Uninstalled => "uninstall",
            Self::Started => "start",
            Self::Stopped => "stop",
        }
    }
}

#[cfg(target_os = "windows")]
struct Snapshot {
    config_path: PathBuf,
    /// What is registered under the service name right now.
    service: ServiceProbe,
}

#[cfg(target_os = "windows")]
fn snapshot() -> Option<Snapshot> {
    with_state(|state| {
        let config_path = state.ctx.config_path.clone();
        let service = probe_service(&config_path);
        Snapshot {
            config_path,
            service,
        }
    })
}

#[cfg(target_os = "windows")]
fn set_busy(busy: bool) {
    with_state(|state| state.busy.set(busy));
    sync();
}

#[cfg(target_os = "windows")]
fn toggle_service() {
    let Some(snapshot) = snapshot() else {
        return;
    };
    let Snapshot {
        config_path,
        service,
        ..
    } = snapshot;
    if with_state(|state| state.busy.get()).unwrap_or(true) {
        tracing::info!("a service operation is already in flight; ignoring the click");
        return;
    }

    let exe = match std::env::current_exe() {
        Ok(exe) => exe,
        Err(e) => {
            tracing::error!("failed to locate the running executable: {e}");
            return;
        }
    };

    let enable = !service.is_ours();
    let t = lang();
    // The tray always registers the default account; naming it in the dialogs is
    // what makes "which account does it run as?" answerable.
    let account = crate::startup::service::ServiceAccount::default().label();

    // Before elevation, and while a person is watching: a registration that could
    // never serve should not raise a UAC prompt. Advisories are folded into the
    // confirmation below, so a warning does not cost a second dialog.
    let advisories = if enable {
        match preflight_for_install(t) {
            InstallPreflight::Proceed { advisories } => advisories,
            InstallPreflight::Refused => return,
        }
    } else {
        None
    };

    let config_display = config_path.display().to_string();
    let base = if enable {
        // The service name is fixed, so enabling this install would otherwise
        // silently repoint another copy's service at this executable and config.
        match &service {
            // The registered file is gone (moved or renamed installation): say so,
            // rather than suggest a copy that still works somewhere.
            ServiceProbe::OtherInstall {
                image_path,
                missing: true,
                ..
            } => t.trf(
                "tray.service_confirm_replace_missing",
                &[
                    ("account", account.as_str()),
                    ("config", config_display.as_str()),
                    ("existing", image_path.as_str()),
                ],
            ),
            ServiceProbe::OtherInstall { image_path, .. } => t.trf(
                "tray.service_confirm_replace",
                &[
                    ("account", account.as_str()),
                    ("config", config_display.as_str()),
                    ("existing", image_path.as_str()),
                ],
            ),
            _ => t.trf(
                "tray.service_confirm_enable",
                &[
                    ("account", account.as_str()),
                    ("config", config_display.as_str()),
                ],
            ),
        }
    } else if service.is_running() {
        // Turning it off while it runs stops the server, with nothing here to
        // take over.
        t.tr("tray.service_confirm_disable_running").to_string()
    } else {
        t.tr("tray.service_confirm_disable").to_string()
    };

    // An elevated install writes the login entry into the elevated account's
    // registry hive: name that account in this confirmation, and treat it as the
    // consent for the write (`service_finished` must not ask again).
    let elevated_install = enable && crate::startup::win32::is_elevated();
    let mut prompt = base;
    if elevated_install {
        let user = std::env::var("USERNAME")
            .unwrap_or_else(|_| t.tr("tray.elevated_user_fallback").to_string());
        prompt = format!(
            "{}\n\n{prompt}",
            t.trf("tray.service_login_elevated", &[("user", user.as_str())])
        );
    }
    if let Some(details) = &advisories {
        prompt = format!(
            "{}\n\n{prompt}",
            t.trf("tray.service_advisories", &[("details", details.as_str())])
        );
    }
    let icon = if advisories.is_some() {
        MB_ICONWARNING
    } else {
        MB_ICONQUESTION
    };
    let answer = show_message(
        t.tr("tray.notify_title"),
        &prompt,
        MB_OKCANCEL | icon | MB_SETFOREGROUND,
    );
    if answer != IDOK {
        tracing::info!("service toggle cancelled");
        sync();
        return;
    }
    if elevated_install {
        with_state(|state| state.login_write_consented.set(true));
    }

    let outcome = if enable {
        ServiceOutcome::Installed
    } else {
        ServiceOutcome::Uninstalled
    };
    launch_helper(&exe, &config_display, outcome);
}

#[cfg(target_os = "windows")]
fn service_action() {
    let Some(snapshot) = snapshot() else {
        return;
    };
    if !snapshot.service.is_ours() {
        return;
    }
    if with_state(|state| state.busy.get()).unwrap_or(true) {
        tracing::info!("a service operation is already in flight; ignoring the click");
        return;
    }
    let exe = match std::env::current_exe() {
        Ok(exe) => exe,
        Err(e) => {
            tracing::error!("failed to locate the running executable: {e}");
            return;
        }
    };
    let outcome = if snapshot.service.is_running() {
        ServiceOutcome::Stopped
    } else {
        ServiceOutcome::Started
    };
    launch_helper(&exe, &snapshot.config_path.display().to_string(), outcome);
}

/// Run `nanofile service <verb> --config …` elevated, disabling the menu while the
/// helper works. The wait thread posts [`ServiceOutcome`] back to the event loop.
#[cfg(target_os = "windows")]
fn launch_helper(exe: &Path, config_display: &str, outcome: ServiceOutcome) {
    // The helper runs elevated and must not show its own (English, technical)
    // dialog: it writes the reason here and the tray shows one localized dialog.
    // The file lives next to the executable so it also works when elevation uses
    // another account (whose temp directory is not this process').
    let error_file = exe
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join(format!("nanofile-service-{}.log", std::process::id()));
    let _ = std::fs::remove_file(&error_file);
    let error_display = error_file.to_string_lossy();
    let parameters = format!(
        "service {} --config {} --error-file {}",
        outcome.verb(),
        crate::startup::cmdline::win_cmd_quote(config_display),
        crate::startup::cmdline::win_cmd_quote(error_display.as_ref())
    );
    with_state(|state| *state.service_error_file.borrow_mut() = Some(error_file));
    set_busy(true);
    match launch_elevated(exe, &parameters) {
        Ok(process) => {
            tracing::info!(action = outcome.verb(), "waiting for the elevated helper");
            // Wait off the loop thread: blocking here would freeze the tray for as
            // long as the helper runs. The handle travels as an integer because a
            // raw pointer is not `Send`.
            let process = process as isize;
            std::thread::spawn(move || {
                let process = process as HANDLE;
                unsafe { WaitForSingleObject(process, 300_000) };
                let mut exit_code = 1u32;
                if unsafe { GetExitCodeProcess(process, &mut exit_code) } == 0 {
                    exit_code = 1;
                }
                unsafe { CloseHandle(process) };
                backend::post_service_result(outcome, exit_code as isize);
            });
        }
        Err(LaunchError::Cancelled) => {
            // The user dismissed the elevation prompt: they know, so no dialog.
            tracing::info!("the elevation prompt was dismissed; nothing was changed");
            with_state(|state| {
                if let Some(path) = state.service_error_file.borrow_mut().take() {
                    let _ = std::fs::remove_file(path);
                }
                state.login_write_consented.set(false);
            });
            set_busy(false);
        }
        Err(LaunchError::Failed(error)) => {
            tracing::error!("failed to start the elevated helper: {error}");
            with_state(|state| {
                if let Some(path) = state.service_error_file.borrow_mut().take() {
                    let _ = std::fs::remove_file(path);
                }
                state.login_write_consented.set(false);
            });
            set_busy(false);
            let t = lang();
            let text = format!("{}\n\n{error}", t.tr("tray.service_launch_failed"));
            show_message(
                t.tr("tray.notify_title"),
                &text,
                MB_OK | MB_ICONERROR | MB_SETFOREGROUND,
            );
        }
    }
}

/// The elevated helper exited: apply the consequence, then say what happened.
#[cfg(target_os = "windows")]
pub(super) fn service_finished(outcome: ServiceOutcome, exit_code: u32) {
    with_state(|state| state.busy.set(false));
    // The helper writes its failure reason here instead of opening its own dialog.
    let error_file = with_state(|state| state.service_error_file.borrow_mut().take()).flatten();
    let helper_detail = error_file.as_ref().and_then(|path| {
        std::fs::read_to_string(path)
            .ok()
            .map(|text| text.trim().to_string())
            .filter(|text| !text.is_empty())
    });
    if let Some(path) = &error_file {
        let _ = std::fs::remove_file(path);
    }
    // The install confirmation may have covered the login entry; consume it on
    // every path so a later operation cannot reuse it.
    let login_consented =
        with_state(|state| state.login_write_consented.replace(false)).unwrap_or(false);
    let t = lang();

    // The helper refused before changing anything (its own preflight): the reason
    // is in the log.
    if exit_code == EXIT_REFUSED as u32 {
        tracing::warn!("the elevated helper refused to change the service");
        sync();
        return;
    }

    // `STILL_ACTIVE`: the helper is still running after five minutes. Only the
    // menu state is refreshed; the helper may still be working.
    if exit_code != 0 && exit_code != STILL_ACTIVE as u32 {
        tracing::error!(exit_code, "the elevated helper failed");
        sync();
        let text = match helper_detail.as_deref() {
            Some(detail) => {
                let detail = crate::logging::dialog_text(detail, 1600);
                t.trf(
                    "tray.service_failed_detail",
                    &[("details", detail.as_str())],
                )
            }
            None => t.tr("tray.service_failed").to_string(),
        };
        show_message(
            t.tr("tray.notify_title"),
            &text,
            MB_OK | MB_ICONERROR | MB_SETFOREGROUND,
        );
        return;
    }

    if exit_code == 0 {
        match outcome {
            ServiceOutcome::Installed => {
                // Start at boot implies the tray at login: write the user's own
                // entry from this process, never from the helper.
                let login_tray = ensure_login_entry(login_consented);
                let account = crate::startup::service::ServiceAccount::default().label();
                let key = if login_tray {
                    "tray.service_installed"
                } else {
                    "tray.service_installed_no_login"
                };
                let text = t.trf(key, &[("account", account.as_str())]);
                show_message(
                    t.tr("tray.notify_title"),
                    &text,
                    MB_OK | MB_ICONINFORMATION | MB_SETFOREGROUND,
                );
            }
            ServiceOutcome::Uninstalled => {
                let key = match retire_login_entry() {
                    LoginCleanup::Removed => "tray.service_removed",
                    LoginCleanup::Kept => "tray.service_removed_login_kept",
                };
                show_message(
                    t.tr("tray.notify_title"),
                    t.tr(key),
                    MB_OK | MB_ICONINFORMATION | MB_SETFOREGROUND,
                );
            }
            // Start/stop is reflected by the status row; a dialog would only be noise.
            ServiceOutcome::Started | ServiceOutcome::Stopped => {}
        }
    }

    sync();
}

/// Ensure the tray starts at login for the account running *this* process, and
/// report whether the entry is active afterwards.
///
/// `consented` means the install confirmation already named the elevated account,
/// so the elevated-registration prompt must not appear a second time.
#[cfg(target_os = "windows")]
fn ensure_login_entry(consented: bool) -> bool {
    let Some(login) = with_state(|state| state.login.clone()) else {
        return false;
    };
    let state = login.state();
    // A live entry of another installation is not ours to replace; the menu warns
    // about it and leaves the click to the user.
    if state.present && state.readable && !state.ours {
        tracing::warn!("the login entry belongs to another installation; not replacing it");
        return false;
    }
    if !consented && !super::login_entry_write_confirmed() {
        tracing::info!("registering the login tray entry was not confirmed");
        return login_tray_active(&login);
    }
    if let Err(e) = login.enable() {
        tracing::warn!("registering the login tray entry failed: {e:#}");
    }
    login_tray_active(&login)
}

#[cfg(target_os = "windows")]
fn login_tray_active(login: &PlatformLogin) -> bool {
    let state = login.state();
    state.present && state.readable && state.ours && state.approved && !state.stale
}

#[cfg(target_os = "windows")]
enum LoginCleanup {
    Removed,
    Kept,
}

#[cfg(target_os = "windows")]
fn retire_login_entry() -> LoginCleanup {
    let Some(login) = with_state(|state| state.login.clone()) else {
        return LoginCleanup::Kept;
    };
    let state = login.state();
    if !state.present {
        return LoginCleanup::Removed;
    }
    if !state.ours {
        return LoginCleanup::Kept;
    }
    match login.retire_ours() {
        Ok(true) => LoginCleanup::Removed,
        Ok(false) => LoginCleanup::Kept,
        Err(e) => {
            tracing::warn!("could not remove the start-at-login entry: {e:#}");
            LoginCleanup::Kept
        }
    }
}

/// The preflight's answer for an install: refused outright, or proceed with any
/// advisory lines folded into the confirmation that follows.
#[cfg(target_os = "windows")]
enum InstallPreflight {
    Proceed { advisories: Option<String> },
    Refused,
}

/// Whether a service registration may proceed, asked of the preflight.
///
/// A `Fail` is refused outright (the message names every path); a `Warn` comes
/// back as advisory text for the install confirmation, so the operator makes one
/// decision instead of dismissing two dialogs. A `Note` is logged only: "the port
/// is in use" is expected while this instance serves it.
#[cfg(target_os = "windows")]
fn preflight_for_install(t: &server::i18n::I18n) -> InstallPreflight {
    use crate::startup::service::{Severity, run_preflight};

    let Some(input) = with_state(|state| state.ctx.preflight.clone()) else {
        return InstallPreflight::Refused;
    };
    let report = run_preflight(&input);
    report.log();

    if report.has(Severity::Fail) {
        let details =
            crate::logging::dialog_text(&report.lines_at_least(Severity::Fail).join("\n"), 1200);
        let text = t.trf(
            "tray.service_preflight_failed",
            &[("details", details.as_str())],
        );
        show_message(
            t.tr("tray.notify_title"),
            &text,
            MB_OK | MB_ICONERROR | MB_SETFOREGROUND,
        );
        return InstallPreflight::Refused;
    }
    let advisories = report.has(Severity::Warn).then(|| {
        crate::logging::dialog_text(&report.lines_at_least(Severity::Warn).join("\n"), 1200)
    });
    InstallPreflight::Proceed { advisories }
}

#[cfg(target_os = "windows")]
enum LaunchError {
    /// The consent dialog was dismissed (`ERROR_CANCELLED`).
    Cancelled,
    Failed(String),
}

/// Run `nanofile <parameters>` elevated, returning the process handle to wait on.
#[cfg(target_os = "windows")]
fn launch_elevated(exe: &Path, parameters: &str) -> Result<HANDLE, LaunchError> {
    // ShellExecuteEx may use COM for the `runas` verb; the init is best effort.
    unsafe {
        CoInitializeEx(std::ptr::null(), 0x2 /* COINIT_APARTMENTTHREADED */)
    };

    let verb = crate::startup::win32::wide("runas");
    let file = crate::startup::win32::wide(&exe.to_string_lossy());
    let parameters = crate::startup::win32::wide(parameters);

    let mut info: SHELLEXECUTEINFOW = unsafe { std::mem::zeroed() };
    info.cbSize = std::mem::size_of::<SHELLEXECUTEINFOW>() as u32;
    info.fMask = SEE_MASK_NOCLOSEPROCESS | SEE_MASK_NOASYNC;
    info.lpVerb = verb.as_ptr();
    info.lpFile = file.as_ptr();
    info.lpParameters = parameters.as_ptr();
    info.nShow = SW_HIDE;

    if unsafe { ShellExecuteExW(&mut info) } == 0 {
        let error = unsafe { GetLastError() };
        if error == ERROR_CANCELLED {
            return Err(LaunchError::Cancelled);
        }
        return Err(LaunchError::Failed(format!("error {error}")));
    }
    if info.hProcess.is_null() {
        return Err(LaunchError::Failed("no process handle".to_string()));
    }
    Ok(info.hProcess)
}

#[cfg(target_os = "windows")]
fn show_message(title: &str, text: &str, flags: u32) -> i32 {
    let title = crate::startup::win32::wide(title);
    let text = crate::startup::win32::wide(text);
    unsafe {
        MessageBoxW(
            std::ptr::null_mut(),
            text.as_ptr(),
            title.as_ptr(),
            flags | MB_TOPMOST,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_menu_id_is_translated_and_unknown_ids_are_ignored() {
        assert_eq!(action_for(ID_OPEN_WEB), Some(TrayAction::OpenWeb));
        assert_eq!(action_for(ID_AUTOSTART), Some(TrayAction::ToggleLogin));
        #[cfg(target_os = "windows")]
        assert_eq!(action_for(ID_SERVICE), Some(TrayAction::ToggleService));
        #[cfg(target_os = "windows")]
        assert_eq!(
            action_for(ID_SERVICE_ACTION),
            Some(TrayAction::ServiceAction)
        );
        assert_eq!(action_for(ID_OPEN_CONFIG), Some(TrayAction::OpenConfig));
        assert_eq!(action_for(ID_QUIT), Some(TrayAction::Quit));
        assert_eq!(action_for("nanofile.unknown"), None);
    }

    /// The status/action rows are inserted and removed while the menu is live; the
    /// Windows backend owns real menu handles even in a console test binary.
    #[cfg(target_os = "windows")]
    #[test]
    fn the_action_row_can_move_in_and_out_of_a_menu() {
        let menu = Menu::new();
        let first = MenuItem::new("first", true, None);
        let last = MenuItem::new("last", true, None);
        menu.append(&first).unwrap();
        menu.append(&last).unwrap();

        let action = MenuItem::new("action", true, None);
        menu.insert(&action, 1).unwrap();
        assert_eq!(menu.items().len(), 3);
        menu.remove(&action).unwrap();
        assert_eq!(menu.items().len(), 2);
    }
}
