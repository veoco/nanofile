use axum::{Form, Json, extract::State, http::StatusCode, response::IntoResponse};
use serde::Deserialize;
use std::sync::Arc;

use crate::AppState;
use crate::middleware::auth::AuthUser;
use crate::service::credential::{CredentialKind, CredentialService};
use crate::service::user::DeviceService;
use base::error::AppError;

/// Formats a Unix timestamp as `YYYY-MM-DD HH:MM` (UTC); bad values become the epoch.
fn format_ts(ts: i64) -> String {
    chrono::DateTime::from_timestamp(ts, 0)
        .unwrap_or_default()
        .format("%Y-%m-%d %H:%M")
        .to_string()
}

/// Body of `DELETE /api2/devices/`: address a *device* by `platform` + `device_id`
/// (what the official clients send), or any other inventory row by `kind` + `id`.
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
/// Every long-lived credential the account holds, one array, each entry tagged with a
/// `kind`: `client`, `browser`, `sync_token` or `device_trust`. A credential's *value* is
/// never included — a `sync_token` row holds a ciphertext this server can decrypt, so
/// only metadata is returned. `client` entries keep their field names and `kind` is an
/// addition, so a client that ignores it sees extra entries rather than a changed shape —
/// hence `/api2/credentials/` is the right endpoint for a *device* screen.
pub async fn list_devices(
    auth: AuthUser,
    State(state): State<Arc<AppState>>,
) -> Result<Json<serde_json::Value>, AppError> {
    let service = CredentialService::new(state.repos.clone());
    let inventory = service
        .inventory(auth.user_id, None, &auth.repo_scope())
        .await?;

    let mut entries: Vec<serde_json::Value> = Vec::new();

    for client in inventory.clients {
        // Stable, non-secret identifier: `token` holds a SHA-256 digest.
        let key = format!("{}:{}", client.platform, client.device_id);
        entries.push(serde_json::json!({
            // A device is a group of rows, addressed by platform + device_id.
            "kind": "client",
            "key": key,
            "platform": client.platform,
            "device_id": client.device_id,
            "device_name": client.device_name,
            "client_version": client.client_version,
            "last_accessed": format_ts(client.created_ts),
            "is_desktop_client": client.is_desktop_client,
        }));
    }

    for browser in inventory.browsers {
        entries.push(serde_json::json!({
            // The addressing kind, not a display name: it goes back to the revoke route.
            "kind": CredentialKind::BrowserSession.id(),
            "key": format!("browser:{}", browser.id),
            "id": browser.id,
            "source": browser.source.id(),
            "client": browser.client,
            "created_at": browser.created_ts,
        }));
    }

    for token in inventory.sync_tokens {
        entries.push(serde_json::json!({
            "kind": CredentialKind::SyncToken.id(),
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
            "kind": CredentialKind::DeviceTrust.id(),
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
/// Revoke one credential: `platform` + `device_id` unlinks everything that device
/// owns, `kind` + `id` revokes the single row the inventory named.
pub async fn unlink_device(
    auth: AuthUser,
    State(state): State<Arc<AppState>>,
    Form(form): Form<UnlinkDeviceForm>,
) -> Result<impl IntoResponse, AppError> {
    // An explicit kind wins, so a browser session with an empty `platform` stays
    // addressable without looking like a device with an empty id.
    if let Some(kind) = form.kind.as_deref() {
        let kind = CredentialKind::from_id(kind)
            .ok_or_else(|| AppError::BadRequest("unknown credential kind".into()))?;
        let id = form
            .id
            .ok_or_else(|| AppError::BadRequest("id is required with kind".into()))?;
        let service = CredentialService::new(state.repos.clone());
        service
            .revoke(auth.user_id, kind, id, &auth.repo_scope())
            .await?;
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
    let result = svc
        .unlink_device(auth.user_id, platform, device_id, &auth.repo_scope())
        .await?;
    Ok((StatusCode::OK, Json(result)))
}
