pub mod events;
pub mod handler;
pub mod manager;

/// Domain-separated signing keys derived from `[notification] private_key`.
/// A **subscription** JWT is handed to every user who can read a repository
/// (via `/seafhttp/repo/{id}/jwt-token`), while an **event** token authorises
/// publishing arbitrary notifications. Signing both with one key let a
/// subscription token validate at `/notification/events` (it only checks the
/// signature and `exp`); a key per purpose means one can never validate as the
/// other. Both derive deterministically from the configured key, so no
/// migration or key distribution is needed.
pub mod keys {
    use sha2::{Digest, Sha256};

    /// Key for repository **subscription** JWTs (WebSocket `subscribe`).
    pub fn subscription_key(root: &str) -> [u8; 32] {
        derive(b"notif-sub-v1:", root)
    }

    /// Key for **event** JWTs posted to `/notification/events`.
    pub fn event_key(root: &str) -> [u8; 32] {
        derive(b"notif-event-v1:", root)
    }

    fn derive(prefix: &[u8], root: &str) -> [u8; 32] {
        let mut hasher = Sha256::new();
        hasher.update(prefix);
        hasher.update(root.as_bytes());
        hasher.finalize().into()
    }
}

use axum::Router;
use std::sync::Arc;

use crate::AppState;

pub fn notification_routes() -> Router<Arc<AppState>> {
    Router::new()
        // WebSocket endpoint; no trailing-slash variant: trim_trailing_slash()
        // 301-redirects, which WebSocket clients cannot follow.
        .route("/notification", axum::routing::get(handler::ws_upgrade))
        .route(
            "/notification/events",
            axum::routing::post(handler::post_event),
        )
        .route("/notification/ping", axum::routing::get(handler::ping))
}
