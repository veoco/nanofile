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
    let mut features = vec![
        "seafile-basic".to_string(),
        "seafile-pro".to_string(),
        "file-search".to_string(),
        "wiki".to_string(),
    ];
    // Advertise the local-browser SSO flow only when the server-side feature
    // is enabled (mirrors seahub's CLIENT_SSO_VIA_LOCAL_BROWSER setting).
    if state.config.server.sso_enabled {
        features.push("client-sso-via-local-browser".to_string());
    }

    let response = ServerInfoResponse {
        version: state.config.server.version.clone(),
        encrypted_library_version: 3,
        features,
    };

    (StatusCode::OK, Json(response))
}
