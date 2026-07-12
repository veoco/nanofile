use axum::{
    Json, Router,
    extract::{Path, State},
};
use serde::Deserialize;
use std::sync::Arc;

use crate::AppState;
use crate::auth::middleware::AuthUser;
use crate::error::AppError;
use crate::sharing::service::share;

#[derive(Deserialize)]
pub struct CreateShareLinkRequest {
    pub repo_id: String,
    pub path: String,
    pub password: Option<String>,
    pub expires_at: Option<i64>,
}

#[derive(Deserialize)]
pub struct BeshareRequest {
    pub share_type: String,
    pub user: String,
    pub permission: Option<String>,
}

/// Request for modifying or deleting a share.
#[derive(Deserialize)]
pub struct ModifyShareRequest {
    pub user: String,
    pub permission: Option<String>,
}

pub fn share_routes() -> Router<Arc<AppState>> {
    Router::new()
        .route(
            "/",
            axum::routing::get(list_share_links).post(create_share_link),
        )
        .route("/{token}", axum::routing::delete(delete_share_link))
}

pub async fn list_share_links(
    auth: AuthUser,
    State(state): State<Arc<AppState>>,
) -> Result<Json<Vec<share::ShareLinkInfo>>, AppError> {
    let infos = share::list_share_links(&state.repos, auth.user_id).await?;
    Ok(Json(infos))
}

pub async fn create_share_link(
    auth: AuthUser,
    State(state): State<Arc<AppState>>,
    Json(req): Json<CreateShareLinkRequest>,
) -> Result<Json<share::ShareLinkInfo>, AppError> {
    let info = share::create_share_link(
        state.db.as_ref(),
        &state.repos,
        &state.config,
        &req.repo_id,
        &req.path,
        req.password.as_deref(),
        req.expires_at,
        auth.user_id,
    )
    .await?;
    Ok(Json(info))
}

pub async fn delete_share_link(
    auth: AuthUser,
    State(state): State<Arc<AppState>>,
    Path(token): Path<String>,
) -> Result<(), AppError> {
    share::delete_share_link(&state.repos, &token, auth.user_id).await?;
    Ok(())
}

/// `POST /api2/beshared-repos/{repo_id}/`
///
/// Shares a repo with another user by adding them as a repo member.
pub async fn beshare_repo(
    auth: AuthUser,
    State(state): State<Arc<AppState>>,
    Path(repo_id): Path<String>,
    Json(req): Json<BeshareRequest>,
) -> Result<Json<serde_json::Value>, AppError> {
    let result = share::beshare_repo(
        state.db.as_ref(),
        &state.repos,
        state.notification_manager.as_ref(),
        &repo_id,
        auth.user_id,
        &req.user,
        req.permission.as_deref(),
    )
    .await?;

    if result.already_shared {
        Ok(Json(
            serde_json::json!({"success": true, "already_shared": true}),
        ))
    } else {
        Ok(Json(serde_json::json!({"success": true})))
    }
}

/// `GET /api2/beshared-repos/{repo_id}/` — list all users shared to this repo.
pub async fn list_share_members(
    auth: AuthUser,
    State(state): State<Arc<AppState>>,
    Path(repo_id): Path<String>,
) -> Result<Json<Vec<share::ShareMember>>, AppError> {
    // Only the repo owner can list share members.
    crate::storage::check_repo_write_permission(state.db.as_ref(), &repo_id, auth.user_id).await?;

    let members = share::list_share_members(&state.repos, &repo_id).await?;
    Ok(Json(members))
}

/// `PUT /api2/beshared-repos/{repo_id}/` — modify a user's share permission.
pub async fn modify_share_permission(
    auth: AuthUser,
    State(state): State<Arc<AppState>>,
    Path(repo_id): Path<String>,
    Json(req): Json<ModifyShareRequest>,
) -> Result<Json<serde_json::Value>, AppError> {
    let new_perm = req
        .permission
        .as_deref()
        .ok_or_else(|| AppError::BadRequest("permission is required".into()))?;

    share::modify_share_permission(
        state.db.as_ref(),
        &state.repos,
        state.notification_manager.as_ref(),
        &repo_id,
        auth.user_id,
        &req.user,
        new_perm,
    )
    .await?;

    Ok(Json(serde_json::json!({"success": true})))
}

/// `DELETE /api2/beshared-repos/{repo_id}/` — remove a user's share.
pub async fn delete_share(
    auth: AuthUser,
    State(state): State<Arc<AppState>>,
    Path(repo_id): Path<String>,
    Json(req): Json<ModifyShareRequest>,
) -> Result<Json<serde_json::Value>, AppError> {
    share::delete_share(
        state.db.as_ref(),
        &state.repos,
        state.notification_manager.as_ref(),
        &repo_id,
        auth.user_id,
        &req.user,
    )
    .await?;

    Ok(Json(serde_json::json!({"success": true})))
}
