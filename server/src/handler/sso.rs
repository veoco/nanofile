use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::HeaderMap;
use std::collections::HashMap;
use std::sync::Arc;

use crate::AppState;
use crate::service::auth::sso::PollResult;
use base::error::AppError;

/// POST /api2/client-sso-link/
///
/// Anonymous per the official protocol: seadroid posts with no body or auth
/// header, and the desktop client sends `shib_*` device params on the query
/// string. Returns the full browser link the client opens.
pub async fn client_sso_link(
    State(state): State<Arc<AppState>>,
    _headers: HeaderMap,
    Query(params): Query<HashMap<String, String>>,
) -> Result<Json<serde_json::Value>, AppError> {
    let svc = state.sso_service();
    let token = svc
        .create_sso_link(
            params.get("shib_platform").cloned(),
            params.get("shib_device_id").cloned(),
            params.get("shib_device_name").cloned(),
            params.get("shib_client_version").cloned(),
        )
        .await?;

    // Never trust the Host header for the SSO link — a forged Host redirects
    // the browser to an attacker-controlled domain and leaks the SSO token.
    // Use the configured site_url directly (no Host fallback, unlike download
    // links). When site_url is still the default, the link points at loopback,
    // which is unreachable for remote clients but never attacker-controlled.
    let link = format!(
        "{}/client-sso/{token}/",
        state.config.server.site_url.trim_end_matches('/')
    );

    Ok(Json(serde_json::json!({ "link": link })))
}

/// GET /api2/client-sso-link/{token}/
///
/// Anonymous status poll. Response shape matches seahub's `ClientSSOLink.get`:
/// `{"status":"waiting"}` while pending, `{"status":"success","username":...,"apiToken":...}`
/// once the browser confirmed (note the camelCase `apiToken`), and
/// `{"status":"error"}` when the completion window expired. Unknown tokens → 404.
pub async fn poll_sso_link(
    State(state): State<Arc<AppState>>,
    Path(token): Path<String>,
) -> Result<Json<serde_json::Value>, AppError> {
    let svc = state.sso_service();
    let result = svc.poll_sso_link(&token).await?;

    let body = match result {
        PollResult::Status(status) => serde_json::json!({ "status": status }),
        PollResult::Success {
            username,
            api_token,
        } => serde_json::json!({
            "status": "success",
            "username": username,
            "apiToken": api_token,
        }),
    };
    Ok(Json(body))
}
