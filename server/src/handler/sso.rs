use axum::Json;
use axum::extract::{ConnectInfo, Path, Query, State};
use axum::http::HeaderMap;
use std::collections::HashMap;
use std::sync::Arc;

use crate::AppState;
use crate::service::auth::sso::PollResult;
use base::error::AppError;

/// Longest accepted `shib_*` device parameter.
///
/// The values are stored verbatim in `sso_login_tokens` and rendered on the
/// confirmation page; the official desktop client sends a hostname and an OS
/// name, so anything longer is not a legitimate client.
const MAX_SSO_PARAM_LEN: usize = 128;

/// POST /api2/client-sso-link/
///
/// Anonymous per the official protocol: seadroid and iOS post with no body and
/// no auth header (so every parameter here is optional), while the desktop
/// client sends `shib_*` device params on the query string. Returns the full
/// browser link the client opens.
///
/// Being anonymous and writing a row per call, this needs a rate limit: without
/// one a trivial loop fills `sso_login_tokens` for an hour at a time and each
/// insert takes the single SQLite writer lock.
pub async fn client_sso_link(
    State(state): State<Arc<AppState>>,
    ConnectInfo(addr): ConnectInfo<std::net::SocketAddr>,
    headers: HeaderMap,
    Query(params): Query<HashMap<String, String>>,
) -> Result<Json<serde_json::Value>, AppError> {
    let client_ip = crate::middleware::effective_client_ip(
        &addr,
        &headers,
        &state.config.server.trusted_proxies,
    );
    if state.auth_limiters.sso_link.is_limited(&client_ip) {
        return Err(AppError::TooManyRequests);
    }
    state.auth_limiters.sso_link.record_attempt(&client_ip);

    let bounded = |key: &str| -> Result<Option<String>, AppError> {
        match params.get(key) {
            None => Ok(None),
            Some(v) if v.len() <= MAX_SSO_PARAM_LEN => Ok(Some(v.clone())),
            Some(_) => Err(AppError::BadRequest(format!("{key} is too long"))),
        }
    };

    let svc = state.sso_service();
    let token = svc
        .create_sso_link(
            bounded("shib_platform")?,
            bounded("shib_device_id")?,
            bounded("shib_device_name")?,
            bounded("shib_client_version")?,
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
