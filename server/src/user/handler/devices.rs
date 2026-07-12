use axum::{Form, Json, extract::State, http::StatusCode, response::IntoResponse};
use serde::Deserialize;
use std::sync::Arc;

use crate::AppState;
use crate::auth::middleware::AuthUser;
use crate::user::service::DeviceService;
use base::error::AppError;

#[derive(Deserialize)]
pub struct UnlinkDeviceForm {
    pub platform: String,
    pub device_id: String,
}

pub fn devices_routes() -> axum::Router<Arc<AppState>> {
    axum::Router::new().route(
        "/devices/",
        axum::routing::get(list_devices).delete(unlink_device),
    )
}

/// GET /api2/devices/
///
/// List all devices connected to the current user's account.
/// Groups api_tokens by (platform, device_id), returning the most
/// recent token for each device.
pub async fn list_devices(
    auth: AuthUser,
    State(state): State<Arc<AppState>>,
) -> Result<Json<Vec<serde_json::Value>>, AppError> {
    let svc = DeviceService::new(state.repos.clone());
    let devices = svc.list_devices(auth.user_id).await?;
    Ok(Json(devices))
}

/// DELETE /api2/devices/
///
/// Unlink (revoke) a device by removing all its API tokens, S2FA device
/// trust tokens, and sync tokens linked via peer_id (= client_id).
/// The client must provide `platform` and `device_id` (form-encoded or JSON).
pub async fn unlink_device(
    auth: AuthUser,
    State(state): State<Arc<AppState>>,
    Form(form): Form<UnlinkDeviceForm>,
) -> Result<impl IntoResponse, AppError> {
    let svc = DeviceService::new(state.repos.clone());
    let result = svc
        .unlink_device(auth.user_id, &form.platform, &form.device_id)
        .await?;
    Ok((StatusCode::OK, Json(result)))
}
