//! Linux tray backend: tray-icon's pure-Rust StatusNotifierItem service (`ksni`).
//!
//! No GTK and no event loop of our own: the icon is a D-Bus service tray-icon
//! drives on its own thread, and the panel calls the menu items from there. muda's
//! menu items and the [`tray_icon::TrayIcon`](super::TrayIcon) are `Rc`-based and
//! only valid on the thread that built them, so both stay here while this thread
//! drains the work those other threads queue for it, one action at a time.

use std::sync::OnceLock;
use std::sync::mpsc::{Sender, channel};

use tokio::sync::mpsc::UnboundedSender;

use super::TrayContext;
use super::actions::TrayAction;

/// Work for the tray thread: a menu click, translated on the D-Bus thread but not
/// run there, a change of the desktop's colour scheme, or a periodic status
/// refresh (the row and tooltip are snapshots).
enum Wake {
    Menu(TrayAction),
    Theme,
    Refresh,
}

static WAKE: OnceLock<Sender<Wake>> = OnceLock::new();

fn wake(event: Wake) {
    match WAKE.get() {
        Some(sender) => {
            let _ = sender.send(event);
        }
        None => tracing::warn!("Tray event loop not running; dropping an event"),
    }
}

/// Queue a menu action for the tray thread.
pub(super) fn defer(action: TrayAction) {
    wake(Wake::Menu(action));
}

/// Ask the tray thread to re-read the desktop theme.
pub(super) fn theme_changed() {
    wake(Wake::Theme);
}

pub(super) fn run(
    ctx: &TrayContext,
    quit_tx: UnboundedSender<crate::TrayCommand>,
    client_mode: bool,
) -> ! {
    let (sender, receiver) = channel();
    if WAKE.set(sender).is_err() {
        tracing::error!("Tray event loop already initialized; menu actions may be lost");
    }

    // Keep the TrayIcon, and with it muda's menu, alive for the loop.
    let tray = match super::create_tray(ctx, quit_tx, client_mode) {
        Ok(tray) => tray,
        Err(e) => {
            tracing::error!("Tray unavailable: {e:#}");
            super::tray_failed(client_mode, &e)
        }
    };
    super::notify::started();
    super::theme::watch();

    // Re-probe on a timer so a service started or stopped outside the menu does
    // not leave the row and tooltip stale; the ksni menu watcher mirrors the
    // resulting item changes.
    if std::thread::Builder::new()
        .name("tray-refresh".into())
        .spawn(|| {
            loop {
                std::thread::sleep(std::time::Duration::from_secs(5));
                wake(Wake::Refresh);
            }
        })
        .is_err()
    {
        tracing::warn!("could not start the tray status refresh timer");
    }

    // The scheme the icon was last rendered for, so an unrelated `SettingChanged`
    // does not rewrite it.
    let mut dark = None;
    while let Ok(event) = receiver.recv() {
        match event {
            Wake::Menu(action) => action.run(),
            Wake::Refresh => super::actions::sync(),
            Wake::Theme => {
                let now = super::theme::is_dark();
                if dark == Some(now) {
                    continue;
                }
                dark = Some(now);
                if let Err(e) = tray.set_icon(Some(super::icon::tray_icon())) {
                    tracing::warn!("Failed to refresh the tray icon: {e}");
                }
            }
        }
    }
    // Unreachable while the static above keeps a sender alive; parking keeps the
    // server running rather than ending the process with no tray left.
    super::park_forever()
}
