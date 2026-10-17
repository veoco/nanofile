//! Linux tray backend: tray-icon on top of GTK + libayatana-appindicator. GTK
//! init, tray creation and the main loop all happen on the process' main thread.

use tokio::sync::mpsc::UnboundedSender;

use super::TrayContext;

pub(super) fn run(
    ctx: &TrayContext,
    quit_tx: UnboundedSender<crate::TrayCommand>,
    client_mode: bool,
) -> ! {
    // A stale DISPLAY (old SSH session) makes GTK init fail; degrade to headless.
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

    // Follow the desktop between light and dark: the handlers hold a clone of the
    // reference-counted icon, so the binding below only covers a failed connect.
    super::theme::watch(&tray);

    gtk::main();
    std::process::exit(0);
}
