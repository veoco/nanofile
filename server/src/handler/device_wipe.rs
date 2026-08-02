use axum::{Json, extract::Form, extract::State};
use std::collections::HashMap;
use std::sync::Arc;

use crate::AppState;
use base::error::AppError;

/// POST /api2/device-wiped/
///
/// Official seafile protocol: the wiped device reports itself anonymously,
/// carrying its own API token in the body (the desktop client sends a form
/// field `token`, no Authorization header). The server invalidates that
/// (user, device)'s sessions. Scoping by the reporting token's owner prevents
/// cross-user session revocation.
pub async fn device_wiped(
    State(state): State<Arc<AppState>>,
    Form(form): Form<HashMap<String, String>>,
) -> Result<Json<serde_json::Value>, AppError> {
    let token = form
        .get("token")
        .ok_or_else(|| AppError::BadRequest("token required".into()))?;

    let token_record = state
        .repos
        .api_token
        .find_by_token(token)
        .await?
        .ok_or_else(|| AppError::BadRequest("invalid token".into()))?;

    let device_id = token_record
        .device_id
        .clone()
        .ok_or_else(|| AppError::BadRequest("token has no device".into()))?;

    let svc = state.sso_service();
    svc.device_wiped(token_record.user_id, &device_id).await?;

    Ok(Json(serde_json::json!({"success": true})))
}
