//! The account's credential inventory over the JSON API.
//!
//! The same data the settings page shows, for scripts and for clients that want
//! to audit themselves. Two things are deliberate:
//!
//! * **Sync tokens are described, never revealed.** Their row holds a
//!   ciphertext this server can decrypt, so returning the model would hand out
//!   the same power as the credential. Only metadata crosses this boundary.
//! * **`GET /api2/devices/` is left alone.** It is the Seafile-compatible
//!   surface the official clients read, and it answers a narrower question
//!   ("devices that identified themselves"). The complete inventory lives here
//!   rather than as extra entries in a response those clients iterate.

use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
};
use serde_json::json;
use std::sync::Arc;

use crate::AppState;
use crate::middleware::auth::AuthUser;
use crate::service::credential::{CredentialKind, CredentialService};
use base::error::AppError;

pub fn credential_routes() -> axum::Router<Arc<AppState>> {
    axum::Router::new()
        .route("/credentials/", axum::routing::get(list_credentials))
        .route(
            "/credentials/{kind}/{id}/",
            axum::routing::delete(revoke_credential),
        )
}

/// GET /api2/credentials/
///
/// Everything long-lived the account holds. Requires `device.read`.
pub async fn list_credentials(
    auth: AuthUser,
    State(state): State<Arc<AppState>>,
) -> Result<Json<serde_json::Value>, AppError> {
    let service = CredentialService::new(state.repos.clone());
    // No "current session" marker: this surface has no browser to point at, and
    // an API key cannot be a session.
    let inventory = service.inventory(auth.user_id, None).await?;

    Ok(Json(json!({
        "client_sessions": inventory
            .clients
            .into_iter()
            .map(|client| json!({
                "platform": client.platform,
                "platform_display": client.platform_display,
                "device_id": client.device_id,
                "device_name": client.device_name,
                "client_version": client.client_version,
                "created_at": client.created_ts,
                "is_desktop_client": client.is_desktop_client,
            }))
            .collect::<Vec<_>>(),
        "browser_sessions": inventory
            .browsers
            .into_iter()
            .map(|browser| json!({
                "id": browser.id,
                "source": browser.source.id(),
                "client": browser.client,
                "created_at": browser.created_ts,
            }))
            .collect::<Vec<_>>(),
        "sync_tokens": inventory
            .sync_tokens
            .into_iter()
            .map(|token| json!({
                "id": token.id,
                "repo_id": token.repo_id,
                "repo_name": token.repo_name,
                "device_name": token.device_name,
                "peer_ip": token.peer_ip,
                "client_version": token.client_version,
                "created_at": token.created_ts,
                "last_sync_at": token.last_sync_ts,
                "expires_at": token.expires_at,
            }))
            .collect::<Vec<_>>(),
        "device_trusts": inventory
            .device_trusts
            .into_iter()
            .map(|trust| json!({
                "id": trust.id,
                "device_id": trust.device_id,
                "device_name": trust.device_name,
                "created_at": trust.created_ts,
                "expires_at": trust.expires_at,
            }))
            .collect::<Vec<_>>(),
        // API keys are managed under `/api2/api-keys/`, which is session-only.
        // This is a count so an inventory can tell whether any exist without
        // opening a second, weaker path to the key surface.
        "api_key_count": inventory.api_key_count,
    })))
}

/// DELETE /api2/credentials/{kind}/{id}/
///
/// Revoke one credential. Requires `device.write`. A client *device* is still
/// revoked through `DELETE /api2/devices/`, because one device owns several
/// credentials that have to go together.
pub async fn revoke_credential(
    auth: AuthUser,
    State(state): State<Arc<AppState>>,
    Path((kind, id)): Path<(String, i32)>,
) -> Result<impl IntoResponse, AppError> {
    let kind = CredentialKind::from_id(&kind)
        .ok_or_else(|| AppError::BadRequest("unknown credential kind".into()))?;
    let service = CredentialService::new(state.repos.clone());
    service.revoke(auth.user_id, kind, id).await?;
    Ok((
        StatusCode::OK,
        Json(json!({"revoked": kind.id(), "id": id})),
    ))
}
