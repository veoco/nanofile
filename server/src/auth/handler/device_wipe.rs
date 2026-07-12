use axum::{Json, extract::State};
use serde::Deserialize;
use std::sync::Arc;

use crate::AppState;
use crate::auth::middleware::AuthUser;
use base::error::AppError;

#[derive(Deserialize)]
pub struct DeviceWipeRequest {
    pub device_id: Option<String>,
    pub platform: Option<String>,
}

/// POST /api2/device-wiped/
///
/// Reports that a device was wiped. Invalidates all tokens
/// associated with that device and logs the event.
pub async fn device_wiped(
    _auth: AuthUser,
    State(state): State<Arc<AppState>>,
    Json(req): Json<DeviceWipeRequest>,
) -> Result<Json<serde_json::Value>, AppError> {
    let svc = state.sso_service();
    svc.device_wiped(
        _auth.user_id,
        req.device_id.as_deref(),
        req.platform.as_deref(),
    )
    .await?;

    Ok(Json(serde_json::json!({"success": true})))
}
