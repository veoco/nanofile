use axum::Json;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use serde::Serialize;

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
pub async fn server_info() -> impl IntoResponse {
    let response = ServerInfoResponse {
        version: "8.0.0".to_string(),
        encrypted_library_version: 3,
        features: vec![
            "seafile-basic".to_string(),
            "seafile-pro".to_string(),
            "file_lock".to_string(),
            "file_tag".to_string(),
            "search".to_string(),
            "thumbnail".to_string(),
            "description".to_string(),
        ],
    };

    (StatusCode::OK, Json(response))
}
