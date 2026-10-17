use axum::Json;
use axum::extract::{ConnectInfo, State};
use std::sync::Arc;

use crate::AppState;
use crate::middleware::auth::AuthUser;
use base::error::AppError;

/// POST /api2/client-login/
///
/// A one-time token for the "view on website" feature, valid 30 seconds (matching
/// Seahub): the client opens `/client-login/?token=...`, which auto-authenticates.
pub async fn client_login(
    auth: AuthUser,
    State(state): State<Arc<AppState>>,
    ConnectInfo(addr): ConnectInfo<std::net::SocketAddr>,
    headers: axum::http::HeaderMap,
) -> Result<Json<serde_json::Value>, AppError> {
    let svc = state.sso_service();
    let token = svc.create_client_login_token(&auth.email).await?;

    // Bind the token to the requesting address. The desktop client and the browser it
    // opens share an egress address, so the web handler can tell a genuine "open in
    // browser" flow from a cross-site link that would silently log the victim into the
    // attacker's account.
    let client_ip = crate::middleware::effective_client_ip(
        &addr,
        &headers,
        &state.config().server.trusted_proxies,
    );
    crate::ui::client_login::remember_issuer(&token, &client_ip);

    Ok(Json(serde_json::json!({"token": token})))
}
