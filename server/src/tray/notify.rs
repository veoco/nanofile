//! Best-effort one-shot notifications (Windows toast / Linux
//! `org.freedesktop.Notifications`). macOS is excluded at compile time: an
//! unsigned, non-bundled binary cannot reliably post them. Failures are logged and
//! ignored — a notification is never a requirement.

#[cfg(not(target_os = "macos"))]
use crate::startup::policy::ServerStatus;

/// The body depends on which process serves: "running in the tray" is all this
/// process guarantees, and with a registered service the server may be stopped.
pub(super) fn started() {
    #[cfg(not(target_os = "macos"))]
    {
        let t = super::lang();
        let key = match super::status() {
            Some(ServerStatus::RunningSelf) => "tray.notify_body",
            Some(status) if status.running() => "tray.notify_client_running",
            Some(ServerStatus::Unknown) => "tray.notify_unknown",
            _ => "tray.notify_client_stopped",
        };
        show(t.tr("tray.notify_title").to_string(), t.tr(key).to_string());
    }
}

/// A failed "Start at login" write: tell the person, without a modal box for a
/// reversible toggle.
pub(super) fn login_failed(detail: &str) {
    #[cfg(not(target_os = "macos"))]
    {
        let t = super::lang();
        let body = t.trf("tray.notify_login_failed", &[("detail", detail)]);
        show(t.tr("tray.notify_title").to_string(), body);
    }
    #[cfg(target_os = "macos")]
    let _ = detail;
}

#[cfg(not(target_os = "macos"))]
fn show(summary: String, body: String) {
    // Spawn so a missing notification service can never delay the tray.
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
