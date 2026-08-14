use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use serde::Serialize;
use std::sync::Arc;

use crate::AppState;

/// Feature strings we advertise in `/api2/server-info/`, cross-checked against
/// the official clients (seafile-client, seadroid, Seafile-iOS) and seahub.
///
/// - `seafile-pro`: NOT a lie — the desktop client gates the Activities and
///   Search tabs on it (`cloud-view.cpp`), the mobile clients gate the
///   Activities tab on it (seadroid `MainActivity`, iOS `isActivityEnabled`),
///   and lock/share menus too. All of those have working backends here
///   (`/api/v2.1/activities/`, search, sync-protocol lock, sharing), so
///   dropping it would hide features that actually work.
/// - `file-search`: gates the desktop Search tab and mobile search. Backend
///   search exists; gated by `file_search_enabled`.
/// - `wiki`: the desktop client ignores it, but seadroid/iOS gate the wiki tab
///   on it. Backend wiki exists; gated by `wiki_enabled`.
/// - `client-sso-via-local-browser`: implemented; gated by `sso_enabled`.
/// - Deliberately NOT advertised: `office-preview` and
///   `disable-sync-with-any-folder` (no backend / off by default in seahub).
#[derive(Serialize)]
pub struct ServerInfoResponse {
    pub version: String,
    pub encrypted_library_version: i32,
    /// Shown by the desktop client in its title bar (seahub's
    /// `DESKTOP_CUSTOM_BRAND`). Only present when configured.
    #[serde(
        rename = "desktop-custom-brand",
        skip_serializing_if = "Option::is_none"
    )]
    pub desktop_custom_brand: Option<String>,
    /// Custom logo path the desktop client joins onto the server URL (seahub's
    /// `DESKTOP_CUSTOM_LOGO`). Only present when configured.
    #[serde(
        rename = "desktop-custom-logo",
        skip_serializing_if = "Option::is_none"
    )]
    pub desktop_custom_logo: Option<String>,
    /// Encrypted-library password hash algorithm, used by desktop/Android
    /// clients when creating encrypted libraries. Only present when configured.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub encrypted_library_pwd_hash_algo: Option<String>,
    /// Parameters for `encrypted_library_pwd_hash_algo`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub encrypted_library_pwd_hash_params: Option<String>,
    pub features: Vec<String>,
}

/// `GET /api2/server-info/`
///
/// Returns server version, encryption version, and supported features.
/// Used by all clients (desktop, mobile) on login to determine capabilities.
/// Public endpoint — no authentication required (matches original seahub).
pub async fn server_info(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let mut features = vec!["seafile-basic".to_string(), "seafile-pro".to_string()];
    // The desktop client's search tab and mobile search key off "file-search".
    if state.config.server.file_search_enabled {
        features.push("file-search".to_string());
    }
    // seadroid/iOS gate the wiki tab on this feature.
    if state.config.server.wiki_enabled {
        features.push("wiki".to_string());
    }
    // Advertise the local-browser SSO flow only when the server-side feature
    // is enabled (mirrors seahub's CLIENT_SSO_VIA_LOCAL_BROWSER setting).
    if state.config.server.sso_enabled {
        features.push("client-sso-via-local-browser".to_string());
    }

    let response = ServerInfoResponse {
        version: state.config.server.version.clone(),
        encrypted_library_version: 3,
        desktop_custom_brand: state.config.server.desktop_custom_brand.clone(),
        desktop_custom_logo: state.config.server.desktop_custom_logo.clone(),
        encrypted_library_pwd_hash_algo: state
            .config
            .server
            .encrypted_library_pwd_hash_algo
            .clone(),
        encrypted_library_pwd_hash_params: state
            .config
            .server
            .encrypted_library_pwd_hash_params
            .clone(),
        features,
    };

    (StatusCode::OK, Json(response))
}
