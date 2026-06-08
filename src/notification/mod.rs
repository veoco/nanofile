pub mod events;
pub mod events_channel;
pub mod handler;
pub mod manager;

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
