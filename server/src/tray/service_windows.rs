//! The tray's "start as a Windows service" item.
//!
//! Registering (or removing) a service needs administrator rights, so the work
//! is handed to a second, elevated copy of this executable
//! (`nanofile service install|uninstall`) through `ShellExecuteExW` with the
//! `runas` verb. That call reports `ERROR_CANCELLED` when the user dismisses the
//! consent dialog, which is what makes "nothing changed" an explicit outcome
//! rather than a silent no-op.
//!
//! Nothing here ever starts or stops the service. Enabling takes effect at the
//! next system start, because handing the listening port over immediately is not
//! reliable on Windows: the port stays in `TIME_WAIT` after the old process'
//! connections are closed, and the service would fail to bind. The confirmation
//! and the result dialog both say so.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use tray_icon::menu::CheckMenuItem;
use windows_sys::Win32::Foundation::{
    CloseHandle, ERROR_CANCELLED, GetLastError, HANDLE, STILL_ACTIVE,
};
use windows_sys::Win32::System::Com::CoInitializeEx;
use windows_sys::Win32::System::Threading::{GetExitCodeProcess, WaitForSingleObject};
use windows_sys::Win32::UI::Shell::{
    SEE_MASK_NOASYNC, SEE_MASK_NOCLOSEPROCESS, SHELLEXECUTEINFOW, ShellExecuteExW,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    IDOK, MB_ICONERROR, MB_ICONINFORMATION, MB_ICONQUESTION, MB_ICONWARNING, MB_OK, MB_OKCANCEL,
    MB_SETFOREGROUND, MB_TOPMOST, MessageBoxW, SW_HIDE,
};

use super::autostart::{Autostart as _, PlatformAutostart};
use super::lang;
use crate::winservice::{ServiceProbe, probe_service};

/// One service toggle at a time: the confirmation and the elevation prompt both
/// sit on this thread, and a second click while a helper is running would race
/// the registration.
static TOGGLE_IN_FLIGHT: AtomicBool = AtomicBool::new(false);

/// What is registered under the service name right now.
pub(super) fn probe(config_path: &Path) -> ServiceProbe {
    probe_service(config_path)
}

/// Quit while another process serves this address.
///
/// In the ordinary tray, Quit stops the server. Client mode must mean the same
/// thing: when the process serving is *this* installation's Windows service,
/// Quit stops that service (it starts again at the next boot, exactly like a
/// quitting tray that the login entry would restart). Only when that is not
/// possible — the service belongs to another installation, or stopping it needs
/// rights this process does not have — does Quit fall back to leaving it
/// running, and then it says so instead of doing it silently.
pub(super) fn quit_in_client_mode() {
    let Some((config_path, _)) = snapshot() else {
        std::process::exit(0);
    };
    let t = lang();
    match probe(&config_path) {
        ServiceProbe::Ours { running: true } => match crate::winservice::stop_service() {
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
                // Cancel keeps the tray (and therefore the way back to the
                // service controls); OK quits the tray alone.
                if answer == IDOK {
                    std::process::exit(0);
                }
            }
        },
        ServiceProbe::OtherInstall { running: true, .. } => {
            // Not ours to stop: it belongs to another installation's executable
            // and config file.
            show_message(
                t.tr("tray.notify_title"),
                t.tr("tray.quit_other_service"),
                MB_OK | MB_ICONINFORMATION | MB_SETFOREGROUND,
            );
            std::process::exit(0);
        }
        // No service of ours is running: whatever holds the address is not this
        // process' to end.
        _ => std::process::exit(0),
    }
}

/// Asks the message loop to run the toggle once muda's dispatch has returned.
pub(super) fn request_toggle() {
    super::backend::request_service_toggle();
}

/// Whether a login-entry change must be refused because *this* installation's
/// service is registered.
///
/// The login item is disabled in that state, so this only fires when the
/// registration changed outside the tray (services.msc, another copy): the two
/// are alternatives, and silently adding a login entry next to the service
/// would produce a second, client-mode tray at every login.
pub(super) fn refuse_login_entry_change() -> bool {
    let Some((config_path, service_item, autostart_item, autostart)) = snapshot_all() else {
        return false;
    };
    if !probe(&config_path).is_ours() {
        return false;
    }
    tracing::info!("the start-at-login entry stays removed while the service is registered");
    let t = lang();
    sync(&config_path, &service_item, &autostart_item, &autostart);
    show_message(
        t.tr("tray.notify_title"),
        t.tr("tray.autostart_superseded_by_service"),
        MB_OK | MB_ICONINFORMATION | MB_SETFOREGROUND,
    );
    true
}

/// The click, deferred to the message loop (muda's synchronous `WM_COMMAND`
/// dispatch still holds a borrow of the clicked item, and both dialogs below
/// pump messages).
pub(super) fn start_toggle() {
    if TOGGLE_IN_FLIGHT.swap(true, Ordering::SeqCst) {
        // Another toggle is waiting on its helper; put the checkbox back where
        // the real state says it belongs and ignore the click.
        if let Some((config_path, item)) = snapshot() {
            item.set_checked(probe(&config_path).is_ours());
        }
        return;
    }

    let Some((config_path, item)) = snapshot() else {
        TOGGLE_IN_FLIGHT.store(false, Ordering::SeqCst);
        return;
    };
    let exe = match std::env::current_exe() {
        Ok(exe) => exe,
        Err(e) => {
            tracing::error!("failed to locate the running executable: {e}");
            item.set_checked(probe(&config_path).is_ours());
            TOGGLE_IN_FLIGHT.store(false, Ordering::SeqCst);
            return;
        }
    };

    let state = probe(&config_path);
    let enable = !state.is_ours();
    let t = lang();
    let config_display = config_path.display().to_string();
    let prompt = if enable {
        // Naming the registration that is about to be replaced matters: the
        // service name is fixed, so enabling this install would otherwise
        // silently repoint another copy's service at this executable and config.
        match &state {
            // The registered file is gone (moved or renamed installation): say
            // so, because "another installation" would suggest a copy that still
            // works somewhere.
            ServiceProbe::OtherInstall {
                image_path,
                missing: true,
                ..
            } => t.trf(
                "tray.service_confirm_replace_missing",
                &[
                    ("config", config_display.as_str()),
                    ("existing", image_path.as_str()),
                ],
            ),
            ServiceProbe::OtherInstall { image_path, .. } => t.trf(
                "tray.service_confirm_replace",
                &[
                    ("config", config_display.as_str()),
                    ("existing", image_path.as_str()),
                ],
            ),
            _ => t.trf(
                "tray.service_confirm_enable",
                &[("config", config_display.as_str())],
            ),
        }
    } else if state.is_running() {
        // Turning it off while it runs stops the server, and the tray in front
        // of the user is not serving anything to take over.
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
        item.set_checked(probe(&config_path).is_ours());
        TOGGLE_IN_FLIGHT.store(false, Ordering::SeqCst);
        return;
    }

    let action = if enable { "install" } else { "uninstall" };
    let parameters = format!(
        "service {action} --config {}",
        win_cmd_quote(&config_display)
    );

    match launch_elevated(&exe, &parameters) {
        Ok(process) => {
            tracing::info!(action, "waiting for the elevated helper");
            // Wait off the loop thread: blocking here would freeze the tray for
            // as long as the helper runs. The handle travels as an integer
            // because a raw pointer is not `Send`.
            let process = process as isize;
            std::thread::spawn(move || {
                let process = process as HANDLE;
                unsafe { WaitForSingleObject(process, 300_000) };
                let mut exit_code = 1u32;
                if unsafe { GetExitCodeProcess(process, &mut exit_code) } == 0 {
                    exit_code = 1;
                }
                unsafe { CloseHandle(process) };
                super::backend::post_service_result(enable, exit_code as isize);
            });
        }
        Err(LaunchError::Cancelled) => {
            // The user dismissed the elevation prompt: they know, so no dialog.
            tracing::info!("the elevation prompt was dismissed; the service was not changed");
            item.set_checked(probe(&config_path).is_ours());
            TOGGLE_IN_FLIGHT.store(false, Ordering::SeqCst);
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
            item.set_checked(probe(&config_path).is_ours());
            TOGGLE_IN_FLIGHT.store(false, Ordering::SeqCst);
        }
    }
}

/// The helper exited: apply the consequence, then say what happened.
///
/// Both automatic-start items are re-synchronised here, never assumed: the
/// install flow removes the login entry and the uninstall flow restores it, so
/// leaving the other item's checkmark as the user last saw it would make the
/// next click do the opposite of what it reads as.
pub(super) fn finish(enable: bool, exit_code: u32) {
    TOGGLE_IN_FLIGHT.store(false, Ordering::SeqCst);
    let Some((config_path, service_item, autostart_item, autostart)) = snapshot_all() else {
        return;
    };
    let t = lang();

    // `STILL_ACTIVE`: the helper is still running after five minutes. Its own
    // dialog owns any error, and what it did may well have succeeded, so only
    // the state is refreshed.
    if exit_code != 0 && exit_code != STILL_ACTIVE as u32 {
        tracing::error!(exit_code, "the elevated helper failed");
        sync(&config_path, &service_item, &autostart_item, &autostart);
        show_message(
            t.tr("tray.notify_title"),
            t.tr("tray.service_failed"),
            MB_OK | MB_ICONERROR | MB_SETFOREGROUND,
        );
        return;
    }

    if exit_code == 0 {
        if enable {
            // The service starts at boot, so the login entry would start a
            // second instance that can only end up as a client-mode tray.
            if let Err(e) = autostart.disable() {
                tracing::warn!("could not remove the start-at-login entry: {e:#}");
            }
            tracing::info!("registered as a Windows service");
            show_message(
                t.tr("tray.notify_title"),
                t.tr("tray.service_installed"),
                MB_OK | MB_ICONINFORMATION | MB_SETFOREGROUND,
            );
        } else {
            // Deliberately *not* re-adding the login entry: "remove the service"
            // means Nanofile no longer starts automatically, and silently writing
            // a startup entry the user did not ask for in that click is worse
            // than starting from an honest blank. `sync` re-enables the menu item
            // so the choice is one click away.
            tracing::info!("the Windows service was removed");
            show_message(
                t.tr("tray.notify_title"),
                t.tr("tray.service_removed"),
                MB_OK | MB_ICONINFORMATION | MB_SETFOREGROUND,
            );
        }
    }

    sync(&config_path, &service_item, &autostart_item, &autostart);
}

/// Put both menu items back in step with the real machine state.
fn sync(
    config_path: &Path,
    service_item: &CheckMenuItem,
    autostart_item: &CheckMenuItem,
    autostart: &PlatformAutostart,
) {
    let ours = probe(config_path).is_ours();
    service_item.set_checked(ours);
    // The login entry is only the way to start automatically while no service
    // of ours is registered; the checkmark still reports the registry as it is.
    autostart_item.set_enabled(!ours);
    autostart_item.set_checked(autostart.is_enabled());
}

enum LaunchError {
    /// The consent dialog was dismissed (`ERROR_CANCELLED`).
    Cancelled,
    Failed(String),
}

/// Run `nanofile <parameters>` elevated, returning the process handle to wait on.
fn launch_elevated(exe: &Path, parameters: &str) -> Result<HANDLE, LaunchError> {
    // ShellExecuteEx may use COM for the `runas` verb; initializing is
    // best-effort (an already-initialized thread returns an error we ignore).
    unsafe {
        CoInitializeEx(std::ptr::null(), 0x2 /* COINIT_APARTMENTTHREADED */)
    };

    let verb = wide("runas");
    let file = wide(&exe.to_string_lossy());
    let parameters = wide(parameters);

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

/// Read the menu state under a short borrow (never across a dialog).
fn snapshot() -> Option<(PathBuf, CheckMenuItem)> {
    super::MENU_STATE.with(|slot| {
        slot.borrow()
            .as_ref()
            .map(|state| (state.ctx.config_path.clone(), state.service_item.clone()))
    })
}

/// The same snapshot, plus everything the completion path has to re-synchronise:
/// the login entry's item *and* its manager, because a successful install
/// removes that entry and a successful uninstall restores it.
fn snapshot_all() -> Option<(PathBuf, CheckMenuItem, CheckMenuItem, PlatformAutostart)> {
    super::MENU_STATE.with(|slot| {
        slot.borrow().as_ref().map(|state| {
            (
                state.ctx.config_path.clone(),
                state.service_item.clone(),
                state.autostart_item.clone(),
                state.autostart.clone(),
            )
        })
    })
}

fn show_message(title: &str, text: &str, flags: u32) -> i32 {
    let title = wide(title);
    let text = wide(text);
    unsafe {
        MessageBoxW(
            std::ptr::null_mut(),
            text.as_ptr(),
            title.as_ptr(),
            flags | MB_TOPMOST,
        )
    }
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Quote one argument for the helper's command line.
fn win_cmd_quote(s: &str) -> String {
    format!("\"{}\"", s.replace('"', "\\\""))
}
