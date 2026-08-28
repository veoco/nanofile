//! Linux tray backend: tray-icon on top of GTK + libayatana-appindicator.
//! GTK must be initialized on the same thread that creates the tray and runs
//! its main loop — this is the process' main thread.

use std::sync::mpsc::Sender;

use super::TrayContext;

pub(super) fn run(ctx: &TrayContext, quit_tx: Sender<crate::TrayCommand>) -> ! {
    // A stale DISPLAY (e.g. from an old SSH session) makes GTK init fail —
    // degrade to headless instead of taking the server down.
    if let Err(e) = gtk::init() {
        tracing::error!("Failed to initialize GTK, running headless: {e}");
        super::park_forever();
    }
    let _tray = match super::create_tray(ctx, quit_tx) {
        Ok(tray) => tray,
        Err(e) => {
            tracing::error!("Tray unavailable, running headless: {e:#}");
            super::park_forever()
        }
    };
    super::notify::started();

    gtk::main();
    std::process::exit(0);
}
