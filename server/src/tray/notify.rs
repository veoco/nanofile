//! Best-effort one-shot "running in the tray" notification shown when the
//! tray appears (Windows toast / Linux `org.freedesktop.Notifications`).
//!
//! macOS is excluded at compile time: an unsigned, non-bundled binary cannot
//! reliably post notifications there. Failures are logged and otherwise
//! ignored — the notification is a convenience, never a requirement.

/// `client_mode` says this tray brought up no server of its own because the
/// address was already served (typically by the Windows service), so the
/// ordinary "Nanofile is running in the system tray" would be misleading.
pub(super) fn started(client_mode: bool) {
    #[cfg(not(target_os = "macos"))]
    {
        let t = super::lang();
        // Spawn so a missing/unresponsive notification service can never
        // delay (or outlive) the tray startup path.
        std::thread::spawn(move || {
            let result = notify_rust::Notification::new()
                .summary(t.tr("tray.notify_title"))
                .body(if client_mode {
                    t.tr("tray.client_mode_body")
                } else {
                    t.tr("tray.notify_body")
                })
                .show();
            if let Err(e) = result {
                tracing::debug!("Tray notification failed: {e}");
            }
        });
    }
    #[cfg(target_os = "macos")]
    {
        let _ = client_mode;
    }
}
