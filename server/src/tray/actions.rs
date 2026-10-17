//! Everything the tray menu *does*.
//!
//! Shape forced by muda: the menu handler runs synchronously inside its
//! `WM_COMMAND` dispatch, which still holds a borrow of the clicked item — so it
//! may not touch that item and may not block on anything that pumps messages (the
//! dialogs do). It only queues a [`TrayAction`]; the Windows message loop drains
//! the queue after the dispatch returns, and every item update and dialog happens
//! there, while other backends run the action directly from their own event loop.
//! Queuing is also why the service toggle needs no in-flight flag: a second click
//! is prevented by disabling the item while the helper runs.

use std::cell::RefCell;
use std::collections::VecDeque;
#[cfg(target_os = "windows")]
use std::path::{Path, PathBuf};

use tokio::sync::mpsc::UnboundedSender;
use tray_icon::menu::{CheckMenuItem, MenuEvent};

use crate::TrayCommand;
use crate::startup::login::{LoginEntry as _, PlatformLogin};
#[cfg(target_os = "windows")]
use crate::startup::policy;

use super::{ID_AUTOSTART, ID_OPEN_CONFIG, ID_OPEN_WEB, ID_QUIT, TrayContext, open_config_file};
#[cfg(target_os = "windows")]
use super::{ID_SERVICE, backend, lang};
#[cfg(target_os = "windows")]
use crate::startup::service::{ServiceProbe, probe_service};
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

/// Everything a menu action needs, plus the items it has to update. Kept in a
/// thread-local because menu items and OS handles are only valid on the event-loop
/// thread.
pub(super) struct MenuState {
    pub(super) ctx: TrayContext,
    pub(super) login: PlatformLogin,
    pub(super) login_item: CheckMenuItem,
    #[cfg(target_os = "windows")]
    pub(super) service_item: CheckMenuItem,
    /// Something else already serves this address, so this process runs no
    /// server and Quit only ends the tray.
    pub(super) client_mode: bool,
    pub(super) quit_tx: UnboundedSender<TrayCommand>,
}

thread_local! {
    static MENU_STATE: RefCell<Option<MenuState>> = const { RefCell::new(None) };
    static PENDING: RefCell<VecDeque<TrayAction>> = const { RefCell::new(VecDeque::new()) };
}

pub(super) fn install(state: MenuState) {
    MENU_STATE.with(|slot| *slot.borrow_mut() = Some(state));
    MenuEvent::set_event_handler(Some(on_menu_event));
}

fn with_state<T>(f: impl FnOnce(&MenuState) -> T) -> Option<T> {
    MENU_STATE.with(|slot| slot.borrow().as_ref().map(f))
}

/// What a menu click asks for: no payloads, since the action reads what it needs
/// from [`MENU_STATE`] when it runs, outside muda's borrow.
#[derive(Debug, Clone, Copy)]
enum TrayAction {
    OpenWeb,
    ToggleLogin,
    #[cfg(target_os = "windows")]
    ToggleService,
    OpenConfig,
    Quit,
}

fn on_menu_event(event: MenuEvent) {
    match event.id.as_ref() {
        ID_OPEN_WEB => defer(TrayAction::OpenWeb),
        ID_AUTOSTART => defer(TrayAction::ToggleLogin),
        #[cfg(target_os = "windows")]
        ID_SERVICE => defer(TrayAction::ToggleService),
        ID_OPEN_CONFIG => defer(TrayAction::OpenConfig),
        ID_QUIT => defer(TrayAction::Quit),
        _ => {}
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
    #[cfg(not(target_os = "windows"))]
    {
        action.run();
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
    fn run(self) {
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
    let Some((login, item)) = with_state(|state| (state.login.clone(), state.login_item.clone()))
    else {
        return;
    };
    // Re-checked because the registration may have changed outside the tray.
    #[cfg(target_os = "windows")]
    {
        let Some(config_path) = with_state(|state| state.ctx.config_path.clone()) else {
            return;
        };
        if refuse_login_entry_change(&config_path) {
            return;
        }
    }
    perform_login_toggle(login, item);
}

fn perform_login_toggle(login: PlatformLogin, item: CheckMenuItem) {
    let result = if login.is_enabled() {
        tracing::info!("Disabling launch at login");
        login.disable()
    } else if !super::login_entry_write_confirmed() {
        // The elevated-registration warning was dismissed; leave the checkbox
        // where the registry actually is.
        tracing::info!("launch-at-login registration cancelled");
        Ok(())
    } else {
        tracing::info!("Enabling launch at login");
        login.enable()
    };
    if let Err(e) = result {
        tracing::error!("Failed to update launch-at-login: {e:#}");
    }
    item.set_checked(login.is_enabled());
}

fn quit() {
    let Some((quit_tx, client_mode)) =
        with_state(|state| (state.quit_tx.clone(), state.client_mode))
    else {
        return;
    };
    if client_mode {
        // Something else serves the address. Quit means stopping the server,
        // which here is this installation's service if that is what listens.
        quit_client();
    } else {
        tracing::info!("Quit requested from tray");
        let _ = quit_tx.send(TrayCommand::Quit);
    }
}

/// Quit from a tray that brought up no server of its own.
fn quit_client() {
    #[cfg(target_os = "windows")]
    quit_in_client_mode();
    #[cfg(not(target_os = "windows"))]
    {
        // No service concept: whatever holds the address is a process the user
        // started, and ending it is not this menu's business.
        tracing::info!("Quit requested from a client-mode tray");
        std::process::exit(0);
    }
}

#[cfg(target_os = "windows")]
struct Snapshot {
    config_path: PathBuf,
    service_item: CheckMenuItem,
    login_item: CheckMenuItem,
    login: PlatformLogin,
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
            service_item: state.service_item.clone(),
            login_item: state.login_item.clone(),
            login: state.login.clone(),
            service,
        }
    })
}

/// Put both menu items back in step with the real machine state, through the same
/// plan the menu was built from, since install and uninstall change the login entry
/// behind the items' back. Re-enabling the service item ends a waiting toggle.
#[cfg(target_os = "windows")]
fn sync(
    service_item: &CheckMenuItem,
    login_item: &CheckMenuItem,
    login: &PlatformLogin,
    config_path: &Path,
) {
    let plan = policy::plan(super::startup_state(config_path, login));
    service_item.set_enabled(true);
    service_item.set_checked(plan.service_checked);
    login_item.set_enabled(plan.login_enabled);
    login_item.set_checked(plan.login_checked);
}

/// Whether a login-entry change must be refused because *this* installation's
/// service is registered. The item is disabled in that state, so this only fires
/// when the registration changed outside the tray: silently adding a login entry
/// next to the service would produce a second, client-mode tray at every login.
#[cfg(target_os = "windows")]
fn refuse_login_entry_change(config_path: &Path) -> bool {
    let Some(snapshot) = snapshot() else {
        return false;
    };
    if !snapshot.service.is_ours() {
        return false;
    }
    tracing::info!("the start-at-login entry stays removed while the service is registered");
    let t = lang();
    sync(
        &snapshot.service_item,
        &snapshot.login_item,
        &snapshot.login,
        config_path,
    );
    show_message(
        t.tr("tray.notify_title"),
        t.tr("tray.autostart_superseded_by_service"),
        MB_OK | MB_ICONINFORMATION | MB_SETFOREGROUND,
    );
    true
}

#[cfg(target_os = "windows")]
fn toggle_service() {
    let Some(snapshot) = snapshot() else {
        return;
    };
    let Snapshot {
        config_path,
        service_item,
        login_item,
        login,
        service,
    } = snapshot;
    // Disabled until the helper has reported back: a second click must not race
    // the registration. `sync` re-enables the item on every terminal path.
    if !service_item.is_enabled() {
        tracing::info!("a service toggle is already in flight; ignoring the click");
        return;
    }
    service_item.set_enabled(false);

    let exe = match std::env::current_exe() {
        Ok(exe) => exe,
        Err(e) => {
            tracing::error!("failed to locate the running executable: {e}");
            sync(&service_item, &login_item, &login, &config_path);
            return;
        }
    };

    let enable = !service.is_ours();
    let t = lang();
    // The tray always registers the default account; naming it in the dialogs is
    // what makes "which account does it run as?" answerable.
    let account = crate::startup::service::ServiceAccount::default().label();

    // Before elevation, and while a person is watching: a registration that could
    // never serve should not raise a UAC prompt.
    if enable && !preflight_allows(t) {
        sync(&service_item, &login_item, &login, &config_path);
        return;
    }

    let config_display = config_path.display().to_string();
    let prompt = if enable {
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

    let answer = show_message(
        t.tr("tray.notify_title"),
        &prompt,
        MB_OKCANCEL | MB_ICONQUESTION | MB_SETFOREGROUND,
    );
    if answer != IDOK {
        tracing::info!("service toggle cancelled");
        sync(&service_item, &login_item, &login, &config_path);
        return;
    }

    let action = if enable { "install" } else { "uninstall" };
    let parameters = format!(
        "service {action} --config {}",
        crate::startup::cmdline::win_cmd_quote(&config_display)
    );

    match launch_elevated(&exe, &parameters) {
        Ok(process) => {
            tracing::info!(action, "waiting for the elevated helper");
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
                backend::post_service_result(enable, exit_code as isize);
            });
        }
        Err(LaunchError::Cancelled) => {
            // The user dismissed the elevation prompt: they know, so no dialog.
            tracing::info!("the elevation prompt was dismissed; the service was not changed");
            sync(&service_item, &login_item, &login, &config_path);
        }
        Err(LaunchError::Failed(error)) => {
            tracing::error!("failed to start the elevated helper: {error}");
            let t = lang();
            let text = format!("{}\n\n{error}", t.tr("tray.service_launch_failed"));
            show_message(
                t.tr("tray.notify_title"),
                &text,
                MB_OK | MB_ICONERROR | MB_SETFOREGROUND,
            );
            sync(&service_item, &login_item, &login, &config_path);
        }
    }
}

/// Whether a service registration may proceed, asked of the preflight.
///
/// A `Fail` is refused outright (the message names every path); a `Warn` is a
/// confirmation, because the operator may know something the check cannot. A
/// `Note` is logged only: "the port is in use" is expected while this instance
/// serves it.
#[cfg(target_os = "windows")]
fn preflight_allows(t: &server::i18n::I18n) -> bool {
    use crate::startup::service::{Severity, run_preflight};

    let Some(input) = with_state(|state| state.ctx.preflight.clone()) else {
        return false;
    };
    let report = run_preflight(&input);
    report.log();

    if report.has(Severity::Fail) {
        let details = report.lines_at_least(Severity::Fail).join("\n");
        let text = t.trf(
            "tray.service_preflight_failed",
            &[("details", details.as_str())],
        );
        show_message(
            t.tr("tray.notify_title"),
            &text,
            MB_OK | MB_ICONERROR | MB_SETFOREGROUND,
        );
        return false;
    }
    if report.has(Severity::Warn) {
        let details = report.lines_at_least(Severity::Warn).join("\n");
        let text = t.trf("tray.service_advisories", &[("details", details.as_str())]);
        return show_message(
            t.tr("tray.notify_title"),
            &text,
            MB_OKCANCEL | MB_ICONWARNING | MB_SETFOREGROUND,
        ) == IDOK;
    }
    true
}

/// The elevated helper exited: apply the consequence, then say what happened.
#[cfg(target_os = "windows")]
pub(super) fn service_finished(enable: bool, exit_code: u32) {
    let Some(snapshot) = snapshot() else {
        return;
    };
    let t = lang();

    // The helper refused before changing anything (its own preflight): the reason
    // is in the log.
    if exit_code == crate::startup::service::EXIT_REFUSED as u32 {
        tracing::warn!("the elevated helper refused to change the service");
        sync(
            &snapshot.service_item,
            &snapshot.login_item,
            &snapshot.login,
            &snapshot.config_path,
        );
        return;
    }

    // `STILL_ACTIVE`: the helper is still running after five minutes. Its own
    // dialog owns any error, so only the menu state is refreshed.
    if exit_code != 0 && exit_code != STILL_ACTIVE as u32 {
        tracing::error!(exit_code, "the elevated helper failed");
        sync(
            &snapshot.service_item,
            &snapshot.login_item,
            &snapshot.login,
            &snapshot.config_path,
        );
        show_message(
            t.tr("tray.notify_title"),
            t.tr("tray.service_failed"),
            MB_OK | MB_ICONERROR | MB_SETFOREGROUND,
        );
        return;
    }

    if exit_code == 0 {
        if enable {
            // The service starts at boot, so the login entry would start a second
            // instance that can only end up as a client-mode tray.
            if let Err(e) = snapshot.login.retire_ours() {
                tracing::warn!("could not remove the start-at-login entry: {e:#}");
            }
            tracing::info!("registered as a Windows service");
            let account = crate::startup::service::ServiceAccount::default().label();
            let text = t.trf("tray.service_installed", &[("account", account.as_str())]);
            show_message(
                t.tr("tray.notify_title"),
                &text,
                MB_OK | MB_ICONINFORMATION | MB_SETFOREGROUND,
            );
        } else {
            // Deliberately *not* re-adding the login entry: "remove the service"
            // means Nanofile no longer starts automatically, so this click must not
            // write a startup entry the user did not ask for.
            tracing::info!("the Windows service was removed");
            show_message(
                t.tr("tray.notify_title"),
                t.tr("tray.service_removed"),
                MB_OK | MB_ICONINFORMATION | MB_SETFOREGROUND,
            );
        }
    }

    sync(
        &snapshot.service_item,
        &snapshot.login_item,
        &snapshot.login,
        &snapshot.config_path,
    );
}

/// Quit while another process serves this address: in the ordinary tray that means
/// stopping the server, so when the process serving it is *this* installation's
/// service, Quit stops that service (it starts again at the next boot). Only when
/// that is impossible — another installation's service, or rights this process
/// lacks — does Quit leave it running, and then it says so instead of silently.
#[cfg(target_os = "windows")]
fn quit_in_client_mode() {
    let Some(config_path) = with_state(|state| state.ctx.config_path.clone()) else {
        std::process::exit(0);
    };
    let t = lang();
    match probe_service(&config_path) {
        ServiceProbe::Ours { running: true } => match crate::startup::service::stop_service() {
            Ok(()) => {
                tracing::info!("service stopped from the tray menu");
                show_message(
                    t.tr("tray.notify_title"),
                    t.tr("tray.quit_service_stopped"),
                    MB_OK | MB_ICONINFORMATION | MB_SETFOREGROUND,
                );
                std::process::exit(0);
            }
            Err(e) => {
                tracing::warn!("could not stop the service from the tray: {e:#}");
                let detail = format!("{e:#}");
                let text = t.trf("tray.quit_service_running", &[("detail", detail.as_str())]);
                let answer = show_message(
                    t.tr("tray.notify_title"),
                    &text,
                    MB_OKCANCEL | MB_ICONWARNING | MB_SETFOREGROUND,
                );
                // Cancel keeps the tray, and with it the way back to the service
                // controls; OK quits the tray alone.
                if answer == IDOK {
                    std::process::exit(0);
                }
            }
        },
        ServiceProbe::OtherInstall { running: true, .. } => {
            // Not ours to stop: it belongs to another installation.
            show_message(
                t.tr("tray.notify_title"),
                t.tr("tray.quit_other_service"),
                MB_OK | MB_ICONINFORMATION | MB_SETFOREGROUND,
            );
            std::process::exit(0);
        }
        // No service of ours is running: whatever holds the address is not ours
        // to end.
        _ => std::process::exit(0),
    }
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
