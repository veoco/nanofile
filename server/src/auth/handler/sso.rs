use axum::Json;
use axum::extract::{Form, Path, State};
use std::collections::HashMap;
use std::sync::Arc;

use crate::AppState;
use crate::auth::middleware::AuthUser;
use crate::error::AppError;

/// POST /api2/client-login/
pub async fn client_login(
    _auth: AuthUser,
    State(state): State<Arc<AppState>>,
) -> Result<Json<serde_json::Value>, AppError> {
    let svc = state.sso_service();
    let token = svc.create_login_token().await?;

    Ok(Json(serde_json::json!({"token": token})))
}

/// POST /api2/client-sso-link/
pub async fn client_sso_link(
    _auth: AuthUser,
    State(state): State<Arc<AppState>>,
    Form(form): Form<HashMap<String, String>>,
) -> Result<Json<serde_json::Value>, AppError> {
    let svc = state.sso_service();
    let result = svc
        .create_sso_link(
            form.get("platform").cloned(),
            form.get("device_id").cloned(),
            form.get("device_name").cloned(),
        )
        .await?;

    Ok(Json(serde_json::json!({
        "link": result.link,
        "token": result.token,
    })))
}

/// GET /api2/client-sso-link/{token}/
pub async fn poll_sso_link(
    _auth: AuthUser,
    State(state): State<Arc<AppState>>,
    Path(token): Path<String>,
) -> Result<Json<serde_json::Value>, AppError> {
    let svc = state.sso_service();
    let api_token = svc.poll_sso_link(&token).await?;

    match api_token {
        Some(token) => Ok(Json(serde_json::json!({
            "status": "done",
            "api_token": token,
        }))),
        None => Ok(Json(serde_json::json!({
            "status": "pending",
        }))),
    }
}
