//! Best-effort one-shot "running in the tray" notification shown when the tray
//! appears (Windows toast / Linux `org.freedesktop.Notifications`). macOS is
//! excluded at compile time: an unsigned, non-bundled binary cannot reliably
//! post them. Failures are logged and ignored — it is never a requirement.

/// `client_mode` means this tray brought up no server of its own — the address was
/// already served, typically by the Windows service — so the usual body misleads.
pub(super) fn started(client_mode: bool) {
    #[cfg(not(target_os = "macos"))]
    {
        let t = super::lang();
        // Spawn so a missing notification service can never delay the tray startup.
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
