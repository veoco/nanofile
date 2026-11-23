use axum::Json;
use axum::extract::State;
use std::sync::Arc;

use crate::AppState;
use crate::middleware::auth::AuthUser;
use base::error::AppError;

/// POST /api2/client-login/
///
/// A one-time token for the "view on website" feature, valid 30 seconds (matching
/// Seahub): the client opens `/client-login/?token=...`. The web entry point does
/// **not** sign the browser in on that GET; it renders a confirmation page whose
/// POST carries an `Origin`, so a link alone cannot switch a browser's account.
pub async fn client_login(
    auth: AuthUser,
    State(state): State<Arc<AppState>>,
) -> Result<Json<serde_json::Value>, AppError> {
    let svc = state.sso_service();
    let token = svc.create_client_login_token(&auth.email).await?;

    Ok(Json(serde_json::json!({ "token": token })))
}
