use axum::{
    Json, Router,
    extract::{Path, State},
};
use serde::Deserialize;
use std::sync::Arc;

use crate::AppState;
use crate::handler::ok_json;
use crate::middleware::auth::AuthUser;
use crate::middleware::repo_extractor::RepoPathWrite;
use crate::service::sharing::share;
use base::error::AppError;

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
    let mut infos =
        share::list_share_links(&state.repos, &state.config.server.site_url, auth.user_id).await?;
    // A share token is a capability URL: never hand one out for a library
    // outside the key's scope.
    let scope = auth.repo_scope();
    infos.retain(|link| scope.allows(&link.repo_id));
    Ok(Json(infos))
}

pub async fn create_share_link(
    auth: AuthUser,
    State(state): State<Arc<AppState>>,
    Json(req): Json<CreateShareLinkRequest>,
) -> Result<Json<share::ShareLinkInfo>, AppError> {
    crate::middleware::ensure_share_links_enabled(&state)?;
    let info = share::create_share_link(
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
) -> Result<Json<serde_json::Value>, AppError> {
    share::delete_share_link(&state.repos, &token, auth.user_id).await?;
    Ok(ok_json())
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
        &state.repos,
        state.notification_manager.as_ref(),
        &repo_id,
        auth.user_id,
        &req.user,
        req.permission.as_deref(),
    )
    .await?;

    state.left_panel_cache.clear_all();

    if result.already_shared {
        Ok(Json(
            serde_json::json!({"success": true, "already_shared": true}),
        ))
    } else {
        Ok(ok_json())
    }
}

/// `GET /api2/beshared-repos/{repo_id}/` — list all users shared to this repo.
///
/// Owner-only. `RepoPathWrite` would admit any `rw` member, which let a
/// collaborator harvest every co-member's email address even though every
/// operation that *changes* membership is owner-only. No official client calls
/// this with GET (the desktop and Android clients only DELETE here), so
/// tightening it costs nothing.
pub async fn list_share_members(
    path: RepoPathWrite,
    State(state): State<Arc<AppState>>,
) -> Result<Json<Vec<share::ShareMember>>, AppError> {
    crate::domain::permission::check_repo_owner(
        state.repos.member.as_ref(),
        &path.repo_id,
        path.user.user_id,
    )
    .await?;

    let members = share::list_share_members(&state.repos, &path.repo_id).await?;
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
        &state.repos,
        state.notification_manager.as_ref(),
        &repo_id,
        auth.user_id,
        &req.user,
        new_perm,
    )
    .await?;

    Ok(ok_json())
}

/// `DELETE /api2/beshared-repos/{repo_id}/` — remove a user's share.
pub async fn delete_share(
    auth: AuthUser,
    State(state): State<Arc<AppState>>,
    Path(repo_id): Path<String>,
    Json(req): Json<ModifyShareRequest>,
) -> Result<Json<serde_json::Value>, AppError> {
    // Resolve the target before the share is deleted so their in-memory
    // capability URLs can be revoked below.
    let target_user_id = state
        .repos
        .user
        .find_by_email(&req.user)
        .await?
        .map(|u| u.id);

    share::delete_share(
        &state.repos,
        state.notification_manager.as_ref(),
        Some(&state.password_manager),
        &repo_id,
        auth.user_id,
        &req.user,
    )
    .await?;

    // Outstanding `/download-api/…` and `/upload-…api/…` URLs for the removed
    // member would otherwise keep working for their remaining TTL.
    if let Some(uid) = target_user_id {
        let revoked = state.token_manager.revoke_user_repo(uid, &repo_id);
        if revoked > 0 {
            tracing::debug!(uid, repo_id, revoked, "revoked access tokens on unshare");
        }
    }

    state.left_panel_cache.clear_all();

    Ok(ok_json())
}
