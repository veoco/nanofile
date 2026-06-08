use axum::{Form, Json, extract::State, http::StatusCode, response::IntoResponse};
use sea_orm::{ColumnTrait, EntityTrait, QueryFilter, QueryOrder};
use serde::Deserialize;
use std::sync::Arc;

use crate::AppState;
use crate::auth::middleware::AuthUser;
use crate::entity::{api_token, s2fa_token, sync_token};
use crate::error::AppError;

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
    let tokens = api_token::Entity::find()
        .filter(api_token::Column::UserId.eq(auth.user_id))
        .filter(api_token::Column::Platform.is_not_null())
        .order_by_desc(api_token::Column::CreatedAt)
        .all(state.db.as_ref())
        .await?;

    let mut seen = std::collections::HashSet::new();
    let mut devices = Vec::new();

    for token in tokens {
        let key = (token.platform.clone(), token.device_id.clone());
        if seen.insert(key) {
            let is_desktop = matches!(
                token.platform.as_deref(),
                Some("windows") | Some("linux") | Some("mac")
            );
            devices.push(serde_json::json!({
                "key": token.token,
                "platform": token.platform,
                "device_id": token.device_id,
                "device_name": token.device_name,
                "client_version": token.client_version,
                "last_accessed": token.created_at,
                "is_desktop_client": is_desktop,
            }));
        }
    }

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
    let db = state.db.as_ref();

    // 1. Delete API tokens for this device (identified by platform + device_id).
    let deleted_api = api_token::Entity::delete_many()
        .filter(api_token::Column::UserId.eq(auth.user_id))
        .filter(api_token::Column::Platform.eq(&form.platform))
        .filter(api_token::Column::DeviceId.eq(&form.device_id))
        .exec(db)
        .await?
        .rows_affected;

    // 2. Delete S2FA device trust tokens (identified by device_id).
    let deleted_s2fa = s2fa_token::Entity::delete_many()
        .filter(s2fa_token::Column::UserId.eq(auth.user_id))
        .filter(s2fa_token::Column::DeviceId.eq(&form.device_id))
        .exec(db)
        .await?
        .rows_affected;

    // 3. Delete sync tokens linked to this device via peer_id (= client_id).
    //    The device_id from API login is the same as seaf-daemon's client_id.
    let deleted_sync = sync_token::Entity::delete_many()
        .filter(sync_token::Column::UserId.eq(auth.user_id))
        .filter(sync_token::Column::PeerId.eq(&form.device_id))
        .exec(db)
        .await?
        .rows_affected;

    Ok((
        StatusCode::OK,
        Json(serde_json::json!({
            "success": true,
            "deleted_api_tokens": deleted_api,
            "deleted_s2fa_tokens": deleted_s2fa,
            "deleted_sync_tokens": deleted_sync,
        })),
    ))
}
