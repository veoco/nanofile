use axum::Json;
use axum::extract::{ConnectInfo, Path, Query, State};
use axum::http::HeaderMap;
use std::collections::HashMap;
use std::sync::Arc;

use crate::AppState;
use crate::service::auth::sso::PollResult;
use base::error::AppError;

/// Longest accepted `shib_*` device parameter: values are stored verbatim and rendered
/// on the confirmation page, and the desktop client sends only a hostname and OS name.
const MAX_SSO_PARAM_LEN: usize = 128;

/// POST /api2/client-sso-link/
///
/// Anonymous per the official protocol: seadroid and iOS post with no body and no auth
/// header (so every parameter is optional) while the desktop client sends `shib_*`
/// params on the query string; returns the browser link. Writing a row per anonymous
/// call, it needs a rate limit or a trivial loop fills `sso_login_tokens` — each insert
/// taking the single SQLite writer lock.
pub async fn client_sso_link(
    State(state): State<Arc<AppState>>,
    ConnectInfo(addr): ConnectInfo<std::net::SocketAddr>,
    headers: HeaderMap,
    Query(params): Query<HashMap<String, String>>,
) -> Result<Json<serde_json::Value>, AppError> {
    let client_ip = crate::middleware::effective_client_ip(
        &addr,
        &headers,
        &state.config().server.trusted_proxies,
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

    // Never trust the Host header for the SSO link — a forged Host redirects the
    // browser to an attacker-controlled domain and leaks the SSO token — so use the
    // configured site_url (no Host fallback). A default site_url points at loopback:
    // unreachable for remote clients, never attacker-controlled.
    let link = format!(
        "{}/client-sso/{token}/",
        state.config().server.site_url.trim_end_matches('/')
    );

    Ok(Json(serde_json::json!({ "link": link })))
}

/// GET /api2/client-sso-link/{token}/
///
/// Anonymous status poll shaped like seahub's `ClientSSOLink.get`: `{"status":"waiting"}`,
/// `{"status":"success","username":...,"apiToken":...}` (camelCase) once the browser
/// confirmed, or `{"status":"error"}` after the window; unknown tokens → 404.
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
