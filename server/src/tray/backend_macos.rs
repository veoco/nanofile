//! macOS tray backend. The tray must be created on the main thread and an
//! event loop must run there, so the process' main thread is handed to
//! NSApplication (as an accessory app: no Dock icon, no menu bar).

use std::sync::mpsc::Sender;

use objc2::MainThreadMarker;
use objc2_app_kit::{NSApplication, NSApplicationActivationPolicy};

use super::TrayContext;

pub(super) fn run(ctx: &TrayContext, quit_tx: Sender<crate::TrayCommand>) -> ! {
    let _tray = match super::create_tray(ctx, quit_tx) {
        Ok(tray) => tray,
        Err(e) => {
            tracing::error!("Tray unavailable, running headless: {e:#}");
            super::park_forever()
        }
    };
    super::notify::started();

    let mtm = MainThreadMarker::new().expect("tray event loop must run on the main thread");
    let app = NSApplication::sharedApplication(mtm);
    // Accessory policy keeps nanofile out of the Dock and the menu bar;
    // it is a server with a tray icon, not a windowed app.
    let _ = app.setActivationPolicy(NSApplicationActivationPolicy::Accessory);
    app.run();
    std::process::exit(0);
}