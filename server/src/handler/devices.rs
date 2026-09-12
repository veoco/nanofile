use axum::{Form, Json, extract::State, http::StatusCode, response::IntoResponse};
use serde::Deserialize;
use std::sync::Arc;

use crate::AppState;
use crate::middleware::auth::AuthUser;
use crate::service::credential::{CredentialKind, CredentialService};
use crate::service::user::DeviceService;
use base::error::AppError;

/// Body of `DELETE /api2/devices/`.
///
/// Two addressing schemes are accepted. A *device* is named by `platform` +
/// `device_id`, which is what the official clients send and what groups a
/// device's credentials together. Everything else the inventory can list
/// (a browser session, a sync token, a 2FA device trust) is named by `kind` +
/// `id`, because it has no device identity to group by.
#[derive(Deserialize)]
pub struct UnlinkDeviceForm {
    #[serde(default)]
    pub platform: Option<String>,
    #[serde(default)]
    pub device_id: Option<String>,
    #[serde(default)]
    pub kind: Option<String>,
    #[serde(default)]
    pub id: Option<i32>,
}

pub fn devices_routes() -> axum::Router<Arc<AppState>> {
    axum::Router::new().route(
        "/devices/",
        axum::routing::get(list_devices).delete(unlink_device),
    )
}

/// GET /api2/devices/
///
/// Everything long-lived the account holds, as one array. Each entry carries a
/// `kind` so a caller can tell them apart:
///
/// * `client` — an application that identified itself, grouped by device;
/// * `browser` — a session held by a web browser;
/// * `sync_token` — one library's sync credential for one device;
/// * `device_trust` — a device allowed to skip the second factor.
///
/// A credential's *value* is never included. A `sync_token` row holds a
/// ciphertext this server can decrypt, so only metadata is returned.
///
/// # Compatibility
///
/// The `client` entries keep the field names they always had, and `kind` is an
/// addition. A client that ignores it therefore sees the extra entries rather
/// than a changed shape — which is why this endpoint, unlike
/// `/api2/credentials/`, is not the right one for a *device* screen to iterate.
pub async fn list_devices(
    auth: AuthUser,
    State(state): State<Arc<AppState>>,
) -> Result<Json<serde_json::Value>, AppError> {
    let service = CredentialService::new(state.repos.clone());
    let inventory = service.inventory(auth.user_id, None).await?;

    let mut entries: Vec<serde_json::Value> = Vec::new();

    for client in inventory.clients {
        // Stable, non-secret identifier: the row's `token` column holds the
        // SHA-256 *digest* of a bearer credential and used to be echoed here.
        let key = format!("{}:{}", client.platform, client.device_id);
        entries.push(serde_json::json!({
            "kind": "client",
            "key": key,
            "platform": client.platform,
            "device_id": client.device_id,
            "device_name": client.device_name,
            "client_version": client.client_version,
            "last_accessed": crate::ui::format_ts(client.created_ts),
            "is_desktop_client": client.is_desktop_client,
        }));
    }

    for browser in inventory.browsers {
        entries.push(serde_json::json!({
            "kind": "browser",
            "key": format!("browser:{}", browser.id),
            "id": browser.id,
            "source": browser.source.id(),
            "client": browser.client,
            "created_at": browser.created_ts,
        }));
    }

    for token in inventory.sync_tokens {
        entries.push(serde_json::json!({
            "kind": "sync_token",
            "key": format!("sync_token:{}", token.id),
            "id": token.id,
            "repo_id": token.repo_id,
            "repo_name": token.repo_name,
            "device_name": token.device_name,
            "peer_ip": token.peer_ip,
            "client_version": token.client_version,
            "created_at": token.created_ts,
            "last_sync_at": token.last_sync_ts,
            "expires_at": token.expires_at,
        }));
    }

    for trust in inventory.device_trusts {
        entries.push(serde_json::json!({
            "kind": "device_trust",
            "key": format!("device_trust:{}", trust.id),
            "id": trust.id,
            "device_id": trust.device_id,
            "device_name": trust.device_name,
            "created_at": trust.created_ts,
            "expires_at": trust.expires_at,
        }));
    }

    Ok(Json(serde_json::Value::Array(entries)))
}

/// DELETE /api2/devices/
///
/// Revoke one credential. With `platform` + `device_id` this is the original
/// device unlink: every credential that device owns goes together. With `kind`
/// + `id` it revokes the single row the inventory named.
pub async fn unlink_device(
    auth: AuthUser,
    State(state): State<Arc<AppState>>,
    Form(form): Form<UnlinkDeviceForm>,
) -> Result<impl IntoResponse, AppError> {
    // An explicit kind wins, so a caller can address a browser session whose
    // `platform` is empty without that looking like a device with an empty id.
    if let Some(kind) = form.kind.as_deref() {
        let kind = CredentialKind::from_id(kind)
            .ok_or_else(|| AppError::BadRequest("unknown credential kind".into()))?;
        let id = form
            .id
            .ok_or_else(|| AppError::BadRequest("id is required with kind".into()))?;
        let service = CredentialService::new(state.repos.clone());
        service.revoke(auth.user_id, kind, id).await?;
        return Ok((
            StatusCode::OK,
            Json(serde_json::json!({"success": true, "revoked": kind.id(), "id": id})),
        ));
    }

    let (platform, device_id) = match (form.platform.as_deref(), form.device_id.as_deref()) {
        (Some(platform), Some(device_id)) if !platform.is_empty() => (platform, device_id),
        _ => {
            return Err(AppError::BadRequest(
                "provide kind+id, or platform+device_id".into(),
            ));
        }
    };

    let svc = DeviceService::new(state.repos.clone());
    let result = svc.unlink_device(auth.user_id, platform, device_id).await?;
    Ok((StatusCode::OK, Json(result)))
}
