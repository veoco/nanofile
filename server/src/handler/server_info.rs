use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use serde::Serialize;
use std::sync::Arc;

use crate::AppState;

/// Feature strings advertised in `/api2/server-info/`, cross-checked against the official
/// clients and seahub. `seafile-pro` is not a lie: clients gate their Activities/Search
/// tabs, lock/share menus and mobile Activities tab on it, and those backends exist here;
/// the share dialog it reaches lists the empty group stub in [`crate::handler::groups`].
/// `file-search` and `client-sso-via-local-browser` are gated by config flags, while
/// `office-preview`/`disable-sync-with-any-folder` are deliberately not advertised.
#[derive(Serialize)]
pub struct ServerInfoResponse {
    pub version: String,
    pub encrypted_library_version: i32,
    /// Desktop client title bar (seahub's `DESKTOP_CUSTOM_BRAND`); configured only.
    #[serde(
        rename = "desktop-custom-brand",
        skip_serializing_if = "Option::is_none"
    )]
    pub desktop_custom_brand: Option<String>,
    /// Custom logo path joined onto the server URL (seahub `DESKTOP_CUSTOM_LOGO`).
    #[serde(
        rename = "desktop-custom-logo",
        skip_serializing_if = "Option::is_none"
    )]
    pub desktop_custom_logo: Option<String>,
    /// Encrypted-library password hash algorithm clients use when creating them.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub encrypted_library_pwd_hash_algo: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub encrypted_library_pwd_hash_params: Option<String>,
    pub features: Vec<String>,
}

/// Public endpoint — no authentication required (matches original seahub).
pub async fn server_info(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let mut features = vec!["seafile-basic".to_string(), "seafile-pro".to_string()];
    if state.config().server.file_search_enabled {
        features.push("file-search".to_string());
    }
    // Mirrors seahub's CLIENT_SSO_VIA_LOCAL_BROWSER setting.
    if state.config().server.sso_enabled {
        features.push("client-sso-via-local-browser".to_string());
    }
    // Lets clients hide sharing entry points; keep in sync with
    // `server.share_download_enabled`: the flag names share links, and the upload
    // half has no client-side capability bit.
    if !state.config().server.share_download_enabled {
        features.push("share-link-disabled".to_string());
    }

    let response = ServerInfoResponse {
        version: state.config().server.version.clone(),
        // Taken verbatim by the clients as the `enc_version` of a library they create,
        // so it is the configured version (2 by default, like seahub's
        // `ENCRYPTED_LIBRARY_VERSION`) — never hard-coded.
        encrypted_library_version: state.config().server.encrypted_library_version,
        desktop_custom_brand: state.config().server.desktop_custom_brand.clone(),
        desktop_custom_logo: state.config().server.desktop_custom_logo.clone(),
        // Advertised only when configured, and then honoured end to end: a client that
        // sees the algorithm generates a `pwd_hash` instead of a `magic`
        // (`seafile_generate_magic_and_random_key`), nanofile stores and re-emits it,
        // and verifies passwords against it. seahub emits
        // `encrypted_library_pwd_hash_params` whenever the algorithm is set, even as
        // `""`; do the same so a client never sees the pair split.
        encrypted_library_pwd_hash_algo: state
            .config()
            .server
            .encrypted_library_pwd_hash_algo
            .clone(),
        encrypted_library_pwd_hash_params: state
            .config()
            .server
            .encrypted_library_pwd_hash_algo
            .as_ref()
            .map(|_| {
                state
                    .config()
                    .server
                    .encrypted_library_pwd_hash_params
                    .clone()
                    .unwrap_or_default()
            }),
        features,
    };

    (StatusCode::OK, Json(response))
}
