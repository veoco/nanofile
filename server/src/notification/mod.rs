pub mod events;
pub mod handler;
pub mod manager;

/// Domain-separated signing keys derived from `[notification] private_key`.
///
/// A **subscription** JWT is handed to every user who can read a repository
/// (via `/seafhttp/repo/{id}/jwt-token`), while an **event** token authorises
/// publishing arbitrary notifications to any repository's subscribers. Signing
/// both with one key meant any user could present their own subscription token
/// to `/notification/events`, because that endpoint only checks the signature
/// and `exp`. Deriving a key per purpose separates the two capabilities: a
/// subscription token can never validate as an event token.
///
/// Both keys are derived deterministically from the configured key, so no
/// migration or key distribution is needed for the server itself.
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
        // WebSocket endpoint for clients to subscribe to repo notifications.
        // NOTE: only register WITHOUT trailing slash because
        // NormalizePathLayer::trim_trailing_slash() does a 301 redirect
        // which WebSocket clients cannot follow.
        .route("/notification", axum::routing::get(handler::ws_upgrade))
        // Event submission endpoint (for server-side / external events)
        .route(
            "/notification/events",
            axum::routing::post(handler::post_event),
        )
        // Health check
        .route("/notification/ping", axum::routing::get(handler::ping))
}
