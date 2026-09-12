//! Legacy per-library WebDAV key endpoints, now served by the unified store.
//!
//! Kept so the library edit dialog and its clients keep working while the
//! unified key page takes over: a key created here is an ordinary `api_keys` row
//! whose capabilities are exactly `webdav.read` (plus `webdav.write` for `rw`)
//! and whose only binding is this library. That is why a migrated or newly
//! created WebDAV key still cannot reach `/api2`.

use axum::{
    Json,
    extract::{Path, State},
};
use serde::Deserialize;
use serde_json::json;
use std::sync::Arc;

use crate::AppState;
use crate::handler::ok_json;
use crate::middleware::auth::AuthUser;
use crate::service::api_key::{ApiKeyService, KeyExpiry, NewApiKey};
use base::error::AppError;

/// Request body for generating a new WebDAV key.
#[derive(Deserialize)]
pub struct CreateWebdavKeyRequest {
    /// Device/usage label, e.g. "MacBook" or "rclone".
    pub name: Option<String>,
    /// Key permission: "rw" (default) or "r" (read-only).
    pub permission: Option<String>,
}

fn capabilities_for(permission: &str) -> Vec<String> {
    if permission == "r" {
        vec!["webdav.read".to_string()]
    } else {
        vec!["webdav.read".to_string(), "webdav.write".to_string()]
    }
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
    // Only "r" and "rw" are valid; anything else falls back to "rw" so a
    // malformed request never silently downgrades a key to read-only.
    let permission = if body.permission.as_deref() == Some("r") {
        "r"
    } else {
        "rw"
    };
    let created = ApiKeyService::create(
        &state.repos,
        &state.config.auth,
        auth.user_id,
        NewApiKey {
            name: Some(body.name.unwrap_or_else(|| "default".to_string())),
            capabilities: capabilities_for(permission),
            all_repos: false,
            repo_permissions: vec![(repo_id.clone(), permission.to_string())],
            // The legacy endpoint never expired its keys; keep that, and let a
            // bounded deployment reject the request instead of silently
            // shortening the lifetime.
            expires: KeyExpiry::Never,
        },
    )
    .await?;

    Ok(Json(json!({
        "key_id": created.view.id,
        "repo_id": repo_id,
        "name": created.view.name,
        "permission": permission,
        "created_at": created.view.created_at,
        "key": created.secret,
    })))
}

/// GET /api2/repos/{repo_id}/webdav-keys/
///
/// List the caller's WebDAV keys for this library. Never includes plaintext.
pub async fn list_webdav_keys(
    auth: AuthUser,
    State(state): State<Arc<AppState>>,
    Path(repo_id): Path<String>,
) -> Result<Json<serde_json::Value>, AppError> {
    let keys = ApiKeyService::list(&state.repos, auth.user_id).await?;
    let items: Vec<serde_json::Value> = keys
        .into_iter()
        .filter(|key| !key.all_repos && key.capabilities.iter().any(|c| c == "webdav.read"))
        .filter_map(|key| {
            let permission = key
                .repos
                .iter()
                .find(|(bound, _)| bound == &repo_id)
                .map(|(_, permission)| permission.clone())?;
            Some(json!({
                "id": key.id,
                "repo_id": repo_id,
                "name": key.name,
                "permission": permission,
                "created_at": key.created_at,
                "last_used_at": key.last_used_at,
            }))
        })
        .collect();
    Ok(Json(json!({ "keys": items })))
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
    let key = state
        .repos
        .api_key
        .find_by_id(key_id)
        .await?
        .ok_or_else(|| AppError::NotFound("WebDAV key not found".into()))?;

    // The key must actually belong to this library.
    let bound_here = state
        .repos
        .api_key
        .list_bindings(key_id)
        .await?
        .iter()
        .any(|binding| binding.repo_id == repo_id);
    if !bound_here {
        return Err(AppError::NotFound("WebDAV key not found".into()));
    }

    if key.user_id != auth.user_id && !is_owner_or_admin(&state, &repo_id, auth.user_id).await? {
        return Err(AppError::Forbidden);
    }

    state.repos.api_key.delete_by_id(key_id).await?;
    Ok(ok_json())
}

/// Whether the caller owns the library or is a server admin.
async fn is_owner_or_admin(
    state: &Arc<AppState>,
    repo_id: &str,
    user_id: i32,
) -> Result<bool, AppError> {
    if let Some(repo) = state.repos.repo.find_by_id(repo_id).await?
        && repo.owner_id == user_id
    {
        return Ok(true);
    }
    Ok(state
        .repos
        .user
        .find_by_id(user_id)
        .await?
        .is_some_and(|user| user.is_admin))
}
