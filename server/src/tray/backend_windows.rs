//! Windows tray backend: tray-icon plus a plain Win32 message loop on the
//! main thread.
//!
//! Re-entrancy contract: muda invokes the shared menu handler synchronously
//! from inside `DispatchMessageW` (its `WM_COMMAND` handling still holds a
//! borrow of the clicked menu item), so the handler must not touch that item
//! and must not block on anything that pumps messages. Work that has to
//! escape those constraints — the launch-at-login toggle, which ends in a
//! `set_checked` on the clicked item and may show a modal elevation dialog —
//! is posted here as a thread message and runs in this loop after the
//! dispatch has returned.

use std::sync::OnceLock;
use std::sync::mpsc::Sender;

use windows_sys::Win32::System::Threading::GetCurrentThreadId;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    DispatchMessageW, GetMessageW, MSG, PostThreadMessageW, TranslateMessage, WM_APP,
};

use super::{TrayContext, perform_autostart_toggle};

/// App-defined thread message carrying a "toggle launch-at-login" request
/// from the menu handler to the message loop.
const WM_APP_TOGGLE_AUTOSTART: u32 = WM_APP + 1;

/// Main thread's id, captured when the loop starts.
static LOOP_THREAD_ID: OnceLock<u32> = OnceLock::new();

/// Called from the menu handler, which runs inside muda's synchronous
/// `WM_COMMAND` dispatch: ask the loop to run the toggle once that dispatch
/// has returned.
pub(super) fn request_autostart_toggle() {
    let Some(&thread_id) = LOOP_THREAD_ID.get() else {
        // The loop has not started (cannot happen for menu events, which are
        // only delivered while it runs). Best effort: run the toggle now.
        tracing::warn!("Tray loop not running, applying launch-at-login toggle directly");
        super::perform_autostart_toggle();
        return;
    };
    if unsafe { PostThreadMessageW(thread_id, WM_APP_TOGGLE_AUTOSTART, 0, 0) } == 0 {
        tracing::error!("Failed to post the launch-at-login toggle request");
    }
}

pub(super) fn run(ctx: &TrayContext, quit_tx: Sender<crate::TrayCommand>) -> ! {
    LOOP_THREAD_ID
        .set(unsafe { GetCurrentThreadId() })
        .expect("tray loop thread id set once");

    // Keep the TrayIcon alive for the lifetime of the loop; dropping it would
    // remove the icon.
    let _tray = match super::create_tray(ctx, quit_tx) {
        Ok(tray) => tray,
        Err(e) => {
            tracing::error!("Tray unavailable, running headless: {e:#}");
            super::park_forever()
        }
    };
    super::notify::started();

    unsafe {
        let mut msg: MSG = std::mem::zeroed();
        while GetMessageW(&mut msg, std::ptr::null_mut(), 0, 0) > 0 {
            // Thread messages (hwnd == NULL) are not routed to a window
            // procedure; handle the deferred tray work here.
            if msg.message == WM_APP_TOGGLE_AUTOSTART && msg.hwnd.is_null() {
                perform_autostart_toggle();
                continue;
            }
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
    // GetMessageW only returns <= 0 on WM_QUIT or error — neither happens
    // normally; the process is exited from the server task.
    std::process::exit(0);
}
