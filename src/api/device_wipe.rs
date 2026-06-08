use axum::{Json, extract::State};
use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};
use serde::Deserialize;
use std::sync::Arc;

use crate::AppState;
use crate::auth::middleware::AuthUser;
use crate::entity::api_token;
use crate::error::AppError;

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
    if let Some(device_id) = &req.device_id {
        // Delete api tokens associated with this device
        api_token::Entity::delete_many()
            .filter(api_token::Column::DeviceId.eq(device_id))
            .exec(state.db.as_ref())
            .await?;

        tracing::info!(
            "device wiped: user_id={}, device_id={:?}, platform={:?}",
            _auth.user_id,
            req.device_id,
            req.platform,
        );
    }

    Ok(Json(serde_json::json!({"success": true})))
}
