//! Unified API-key management endpoints.
//!
//! These routes are session-only (see the capability table): an API key must
//! never be able to mint another key, or it could widen its own access.

use axum::{
    Json,
    extract::{Path, State},
};
use serde::Deserialize;
use serde_json::json;
use std::sync::Arc;

use crate::AppState;
use crate::middleware::auth::AuthUser;
use crate::service::api_key::{
    ApiKeyService, ApiKeyUpdate, ApiKeyView, KeyExpiry, NewApiKey, catalog,
};
use base::error::AppError;

#[derive(Deserialize)]
pub struct RepoPermissionBody {
    pub repo_id: String,
    pub permission: String,
}

#[derive(Deserialize)]
pub struct CreateApiKeyRequest {
    pub name: Option<String>,
    pub capabilities: Vec<String>,
    #[serde(default)]
    pub all_repos: bool,
    #[serde(default)]
    pub repo_permissions: Vec<RepoPermissionBody>,
    pub expires_in_days: Option<u64>,
    pub expires_at: Option<i64>,
    #[serde(default)]
    pub never: bool,
}

#[derive(Deserialize)]
pub struct UpdateApiKeyRequest {
    pub name: Option<String>,
    pub capabilities: Option<Vec<String>>,
    pub all_repos: Option<bool>,
    pub repo_permissions: Option<Vec<RepoPermissionBody>>,
    pub expires_in_days: Option<u64>,
    pub expires_at: Option<i64>,
    pub never: Option<bool>,
}

fn repo_permissions(body: Vec<RepoPermissionBody>) -> Vec<(String, String)> {
    body.into_iter()
        .map(|entry| (entry.repo_id, entry.permission))
        .collect()
}

/// Resolve exactly one lifetime choice.
///
/// Requiring an explicit choice keeps "never" from being the silent default of
/// a client that forgot the field.
fn expiry_of(
    never: bool,
    expires_in_days: Option<u64>,
    expires_at: Option<i64>,
) -> Result<KeyExpiry, AppError> {
    let mut choices = 0;
    choices += usize::from(never);
    choices += usize::from(expires_in_days.is_some());
    choices += usize::from(expires_at.is_some());
    match choices {
        1 if never => Ok(KeyExpiry::Never),
        1 => match (expires_in_days, expires_at) {
            (Some(days), None) => Ok(KeyExpiry::InDays(days)),
            (None, Some(timestamp)) => Ok(KeyExpiry::At(timestamp)),
            _ => unreachable!("exactly one lifetime field is present"),
        },
        _ => Err(AppError::BadRequest(
            "specify exactly one of never, expires_in_days or expires_at".into(),
        )),
    }
}

fn view_json(view: &ApiKeyView) -> serde_json::Value {
    json!({
        "id": view.id,
        "name": view.name,
        "key_prefix": view.key_prefix,
        "capabilities": view.capabilities,
        "all_repos": view.all_repos,
        "repos": view
            .repos
            .iter()
            .map(|(repo_id, permission)| json!({"repo_id": repo_id, "permission": permission}))
            .collect::<Vec<_>>(),
        "created_at": view.created_at,
        "expires_at": view.expires_at,
        "last_used_at": view.last_used_at,
    })
}

/// GET /api2/api-keys/
pub async fn list_api_keys(
    auth: AuthUser,
    State(state): State<Arc<AppState>>,
) -> Result<Json<serde_json::Value>, AppError> {
    let keys = ApiKeyService::list(&state.repos, auth.user_id).await?;
    let items: Vec<serde_json::Value> = keys.iter().map(view_json).collect();
    Ok(Json(json!({ "keys": items })))
}

/// POST /api2/api-keys/
///
/// The plaintext secret is returned exactly once; it is stored hashed and can
/// never be retrieved again.
pub async fn create_api_key(
    auth: AuthUser,
    State(state): State<Arc<AppState>>,
    Json(body): Json<CreateApiKeyRequest>,
) -> Result<Json<serde_json::Value>, AppError> {
    let expires = expiry_of(body.never, body.expires_in_days, body.expires_at)?;
    let created = ApiKeyService::create(
        &state.repos,
        &state.config.auth,
        auth.user_id,
        NewApiKey {
            name: body.name,
            capabilities: body.capabilities,
            all_repos: body.all_repos,
            repo_permissions: repo_permissions(body.repo_permissions),
            expires,
        },
    )
    .await?;

    let mut response = view_json(&created.view);
    response["key"] = json!(created.secret);
    Ok(Json(response))
}

/// GET /api2/api-keys/{key_id}/
pub async fn get_api_key(
    auth: AuthUser,
    State(state): State<Arc<AppState>>,
    Path(key_id): Path<i32>,
) -> Result<Json<serde_json::Value>, AppError> {
    let view = ApiKeyService::get(&state.repos, auth.user_id, key_id).await?;
    Ok(Json(view_json(&view)))
}

/// PUT /api2/api-keys/{key_id}/
pub async fn update_api_key(
    auth: AuthUser,
    State(state): State<Arc<AppState>>,
    Path(key_id): Path<i32>,
    Json(body): Json<UpdateApiKeyRequest>,
) -> Result<Json<serde_json::Value>, AppError> {
    // On update, omitting every lifetime field means "leave the expiry alone".
    let expires =
        if body.never.is_none() && body.expires_in_days.is_none() && body.expires_at.is_none() {
            None
        } else {
            Some(expiry_of(
                body.never.unwrap_or(false),
                body.expires_in_days,
                body.expires_at,
            )?)
        };
    let view = ApiKeyService::update(
        &state.repos,
        &state.config.auth,
        auth.user_id,
        key_id,
        ApiKeyUpdate {
            name: body.name,
            capabilities: body.capabilities,
            all_repos: body.all_repos,
            repo_permissions: body.repo_permissions.map(repo_permissions),
            expires,
        },
    )
    .await?;
    Ok(Json(view_json(&view)))
}

/// DELETE /api2/api-keys/{key_id}/
pub async fn delete_api_key(
    auth: AuthUser,
    State(state): State<Arc<AppState>>,
    Path(key_id): Path<i32>,
) -> Result<Json<serde_json::Value>, AppError> {
    ApiKeyService::revoke(&state.repos, auth.user_id, key_id).await?;
    Ok(Json(json!({ "success": true })))
}

/// GET /api2/api-keys/catalog/
///
/// The capability catalog, the presets and the configured lifetime options, so
/// a client can render the same picker the settings page does.
pub async fn api_key_catalog(
    auth: AuthUser,
    State(state): State<Arc<AppState>>,
) -> Result<Json<serde_json::Value>, AppError> {
    let is_admin = state
        .repos
        .user
        .find_by_id(auth.user_id)
        .await?
        .map(|user| user.is_admin)
        .unwrap_or(false);
    let (entries, presets) = catalog(is_admin);
    Ok(Json(json!({
        "capabilities": entries
            .iter()
            .map(|entry| json!({
                "id": entry.id,
                "domain": entry.domain,
                "write": entry.write,
            }))
            .collect::<Vec<_>>(),
        "presets": presets
            .iter()
            .map(|preset| json!({
                "id": preset.id,
                "capabilities": preset.capabilities,
            }))
            .collect::<Vec<_>>(),
        "ttl_presets_days": state.config.auth.api_key_ttl_presets_days,
        "max_ttl_days": state.config.auth.api_key_max_ttl_days,
    })))
}
