use axum::Json;
use axum::extract::State;
use std::sync::Arc;

use crate::AppState;
use crate::auth::middleware::AuthUser;
use base::error::AppError;

/// POST /api2/client-login/
///
/// Generate a one-time token for the "view on website" feature.
/// The sync client calls this to get a token, then opens a browser
/// to /client-login/?token=... which auto-authenticates the user.
/// Token is valid for 30 seconds (matching Seahub behavior).
pub async fn client_login(
    auth: AuthUser,
    State(state): State<Arc<AppState>>,
) -> Result<Json<serde_json::Value>, AppError> {
    let svc = state.sso_service();
    let token = svc.create_client_login_token(&auth.email).await?;

    Ok(Json(serde_json::json!({"token": token})))
}
