//! Best-effort one-shot notification shown when the tray appears (Windows toast /
//! Linux `org.freedesktop.Notifications`). macOS is excluded at compile time: an
//! unsigned, non-bundled binary cannot reliably post them. Failures are logged and
//! ignored — it is never a requirement.

#[cfg(not(target_os = "macos"))]
use crate::startup::policy::ServerStatus;

/// The body depends on which process serves: "running in the tray" is all this
/// process guarantees, and with a registered service the server may be stopped.
pub(super) fn started() {
    #[cfg(not(target_os = "macos"))]
    {
        let t = super::lang();
        let summary = t.tr("tray.notify_title").to_string();
        let key = match super::status() {
            Some(ServerStatus::RunningSelf) => "tray.notify_body",
            Some(status) if status.running() => "tray.notify_client_running",
            Some(ServerStatus::Unknown) => "tray.notify_unknown",
            _ => "tray.notify_client_stopped",
        };
        let body = t.tr(key).to_string();
        // Spawn so a missing notification service can never delay the tray startup.
        std::thread::spawn(move || {
            let result = notify_rust::Notification::new()
                .summary(&summary)
                .body(&body)
                .show();
            if let Err(e) = result {
                tracing::debug!("Tray notification failed: {e}");
            }
        });
    }
    #[cfg(target_os = "macos")]
    {
        // No reliable notification without a signed bundle.
    }
}
