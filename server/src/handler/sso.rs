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
    headers: HeaderMap,
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

    let host = headers.get("host").and_then(|v| v.to_str().ok());
    let base = state.config.server.download_url_base(host);
    let link = format!("{}/client-sso/{token}/", base.trim_end_matches('/'));

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
