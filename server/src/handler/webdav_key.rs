use axum::{
    Json,
    extract::{Path, State},
};
use serde::Deserialize;
use std::sync::Arc;

use crate::AppState;
use crate::middleware::auth::AuthUser;
use crate::service::repo::webdav_key::WebdavKeyService;
use base::error::AppError;

/// Request body for generating a new WebDAV key.
#[derive(Deserialize)]
pub struct CreateWebdavKeyRequest {
    /// Device/usage label, e.g. "MacBook" or "rclone".
    pub name: Option<String>,
}

/// POST /api2/repos/{repo_id}/webdav-keys/
///
/// Generate a new WebDAV key. The plaintext key is returned exactly once
/// (it is stored hashed and cannot be retrieved again).
pub async fn create_webdav_key(
    auth: AuthUser,
    State(state): State<Arc<AppState>>,
    Path(repo_id): Path<String>,
    Json(body): Json<CreateWebdavKeyRequest>,
) -> Result<Json<serde_json::Value>, AppError> {
    let name = body.name.unwrap_or_else(|| "default".to_string());
    let (model, key) =
        WebdavKeyService::generate_key(&state.repos, &repo_id, auth.user_id, &name).await?;
    Ok(Json(serde_json::json!({
        "key_id": model.id,
        "repo_id": model.repo_id,
        "name": model.name,
        "created_at": model.created_at,
        "key": key,
    })))
}

/// GET /api2/repos/{repo_id}/webdav-keys/
///
/// List WebDAV keys. Never includes plaintext keys.
pub async fn list_webdav_keys(
    auth: AuthUser,
    State(state): State<Arc<AppState>>,
    Path(repo_id): Path<String>,
) -> Result<Json<serde_json::Value>, AppError> {
    let keys = WebdavKeyService::list_keys(&state.repos, &repo_id, auth.user_id).await?;
    let items: Vec<serde_json::Value> = keys
        .into_iter()
        .map(|k| {
            serde_json::json!({
                "id": k.id,
                "repo_id": k.repo_id,
                "name": k.name,
                "created_at": k.created_at,
                "last_used_at": k.last_used_at,
            })
        })
        .collect();
    Ok(Json(serde_json::json!({ "keys": items })))
}

/// DELETE /api2/repos/{repo_id}/webdav-keys/{key_id}/
///
/// Delete a single WebDAV key. Users may delete their own keys; owners and
/// admins may delete any key in the repo.
pub async fn delete_webdav_key(
    auth: AuthUser,
    State(state): State<Arc<AppState>>,
    Path((repo_id, key_id)): Path<(String, i32)>,
) -> Result<Json<serde_json::Value>, AppError> {
    WebdavKeyService::delete_key(&state.repos, &repo_id, auth.user_id, key_id).await?;
    Ok(Json(serde_json::json!({ "success": true })))
}
