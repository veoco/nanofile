//! Windows tray backend: tray-icon plus a plain Win32 message loop on the main thread.
//!
//! Re-entrancy contract: muda invokes the shared menu handler synchronously
//! from inside `DispatchMessageW`, still holding a borrow of the clicked item,
//! so the handler must not touch that item nor block on anything that pumps
//! messages; it only queues an action ([`super::actions`]), drained by this
//! loop after the dispatch returned — where dialogs and `set_checked` run.

use std::sync::OnceLock;
use tokio::sync::mpsc::UnboundedSender;

use windows_sys::Win32::System::Threading::GetCurrentThreadId;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    DispatchMessageW, GetMessageW, MSG, PostThreadMessageW, TranslateMessage, WM_APP,
    WM_SETTINGCHANGE,
};

use super::TrayContext;

/// "Run the queued menu actions": confirmations and elevation stay out of muda's dispatch.
const WM_APP_TRAY_ACTION: u32 = WM_APP + 1;
/// "The elevated helper exited": `wparam` is 1 for an install and 0 for an
/// uninstall, `lparam` is its exit code.
const WM_APP_SERVICE_RESULT: u32 = WM_APP + 2;

static LOOP_THREAD_ID: OnceLock<u32> = OnceLock::new();

fn post(message: u32, wparam: usize, lparam: isize) -> bool {
    let Some(&thread_id) = LOOP_THREAD_ID.get() else {
        return false;
    };
    unsafe { PostThreadMessageW(thread_id, message, wparam, lparam) != 0 }
}

/// Ask the loop to run the queued actions once muda's dispatch returns.
pub(super) fn post_action() -> bool {
    post(WM_APP_TRAY_ACTION, 0, 0)
}

/// Called from the helper-waiting thread once the elevated copy exited.
pub(super) fn post_service_result(enable: bool, exit_code: isize) -> bool {
    post(WM_APP_SERVICE_RESULT, usize::from(enable), exit_code)
}

pub(super) fn run(
    ctx: &TrayContext,
    quit_tx: UnboundedSender<crate::TrayCommand>,
    client_mode: bool,
) -> ! {
    LOOP_THREAD_ID
        .set(unsafe { GetCurrentThreadId() })
        .expect("tray loop thread id set once");

    // Keep the TrayIcon alive for the loop: dropping it would remove the icon.
    let tray = match super::create_tray(ctx, quit_tx, client_mode) {
        Ok(tray) => tray,
        Err(e) => {
            tracing::error!("Tray unavailable, running headless: {e:#}");
            super::park_forever()
        }
    };
    super::notify::started(client_mode);

    unsafe {
        let mut msg: MSG = std::mem::zeroed();
        while GetMessageW(&mut msg, std::ptr::null_mut(), 0, 0) > 0 {
            // Thread messages (hwnd == NULL) never reach a window procedure.
            if msg.hwnd.is_null() {
                match msg.message {
                    WM_APP_TRAY_ACTION => {
                        super::actions::drain();
                        continue;
                    }
                    WM_APP_SERVICE_RESULT => {
                        super::actions::service_finished(msg.wParam != 0, msg.lParam as u32);
                        continue;
                    }
                    _ => {}
                }
            }
            // The shell broadcasts this when the taskbar flips between light and
            // dark (among other settings). GetMessageW with a null filter also
            // picks it up for the tray's hidden window, so re-read the theme here;
            // refreshing on an unrelated setting change is cheap and idempotent.
            if msg.message == WM_SETTINGCHANGE
                && let Err(e) = tray.set_icon(Some(super::icon::tray_icon()))
            {
                tracing::warn!("Failed to refresh the tray icon: {e}");
            }
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
    // GetMessageW returns <= 0 only on WM_QUIT or error; the server task exits.
    std::process::exit(0);
}
