//! Linux tray backend: tray-icon on top of GTK + libayatana-appindicator.
//! GTK must be initialized on the same thread that creates the tray and runs
//! its main loop — this is the process' main thread.

use tokio::sync::mpsc::UnboundedSender;

use super::TrayContext;

pub(super) fn run(
    ctx: &TrayContext,
    quit_tx: UnboundedSender<crate::TrayCommand>,
    client_mode: bool,
) -> ! {
    // A stale DISPLAY (e.g. from an old SSH session) makes GTK init fail —
    // degrade to headless instead of taking the server down.
    if let Err(e) = gtk::init() {
        tracing::error!("Failed to initialize GTK, running headless: {e}");
        super::park_forever();
    }
    let tray = match super::create_tray(ctx, quit_tx, client_mode) {
        Ok(tray) => tray,
        Err(e) => {
            tracing::error!("Tray unavailable, running headless: {e:#}");
            super::park_forever()
        }
    };
    super::notify::started(client_mode);

    // Follow the desktop between light and dark while we run. The handlers hold
    // a clone of the (reference-counted) tray icon, so this binding only has to
    // cover the case where the theme signals could not be connected.
    super::theme::watch(&tray);

    gtk::main();
    std::process::exit(0);
}
