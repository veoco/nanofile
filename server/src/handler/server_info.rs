use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use serde::Serialize;
use std::sync::Arc;

use crate::AppState;

#[derive(Serialize)]
pub struct ServerInfoResponse {
    pub version: String,
    pub encrypted_library_version: i32,
    pub features: Vec<String>,
}

/// `GET /api2/server-info/`
///
/// Returns server version, encryption version, and supported features.
/// Used by all clients (desktop, mobile) on login to determine capabilities.
/// Public endpoint — no authentication required (matches original seahub).
pub async fn server_info(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let response = ServerInfoResponse {
        version: state.config.server.version.clone(),
        encrypted_library_version: 3,
        // Official feature strings recognized by the clients (see seahub's
        // ServerInfoView). `file-search` gates the search tab in both clients.
        //
        // `client-sso-via-local-browser` is intentionally NOT advertised: the
        // local-browser SSO flow is not yet wire-compatible with the official
        // clients (the browser pages `/client-sso/{token}/` and
        // `/client-sso/{token}/complete/` are missing and the poll response
        // differs — see handler/sso.rs). Advertising it would make desktop
        // clients enter a broken SSO flow instead of falling back to normal
        // login, so it stays off until the flow is actually implemented.
        features: vec![
            "seafile-basic".to_string(),
            "seafile-pro".to_string(),
            "file-search".to_string(),
            "wiki".to_string(),
        ],
    };

    (StatusCode::OK, Json(response))
}
