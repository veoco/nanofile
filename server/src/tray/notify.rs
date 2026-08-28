//! Best-effort one-shot "running in the tray" notification shown when the
//! tray appears (Windows toast / Linux `org.freedesktop.Notifications`).
//!
//! macOS is excluded at compile time: an unsigned, non-bundled binary cannot
//! reliably post notifications there. Failures are logged and otherwise
//! ignored — the notification is a convenience, never a requirement.

pub(super) fn started() {
    #[cfg(not(target_os = "macos"))]
    {
        // Spawn so a missing/unresponsive notification service can never
        // delay (or outlive) the tray startup path.
        std::thread::spawn(|| {
            let result = notify_rust::Notification::new()
                .summary("Nanofile")
                .body("nanofile server is running in the tray")
                .show();
            if let Err(e) = result {
                tracing::debug!("Tray notification failed: {e}");
            }
        });
    }
}
