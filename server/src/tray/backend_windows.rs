//! Windows tray backend: tray-icon plus a plain Win32 message loop on the
//! main thread. muda invokes the shared menu handler from inside
//! `DispatchMessageW`, so no polling is needed.

use std::sync::mpsc::Sender;

use windows_sys::Win32::UI::WindowsAndMessaging::{
    DispatchMessageW, GetMessageW, MSG, TranslateMessage,
};

use super::TrayContext;

pub(super) fn run(ctx: &TrayContext, quit_tx: Sender<crate::TrayCommand>) -> ! {
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
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
    // GetMessageW only returns <= 0 on WM_QUIT or error — neither happens
    // normally; the process is exited from the server task.
    std::process::exit(0);
}
