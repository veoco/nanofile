use axum::{
    Json, Router,
    extract::{Path, State},
};
use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

use crate::AppState;
use crate::auth::middleware::AuthUser;
use crate::auth::token::generate_share_link_token;
use crate::entity::share_link;
use crate::error::AppError;

#[derive(Deserialize)]
pub struct CreateShareLinkRequest {
    pub repo_id: String,
    pub path: String,
    pub password: Option<String>,
    pub expires_at: Option<i64>,
}

#[derive(Serialize)]
pub struct ShareLinkInfo {
    pub token: String,
    pub link: String,
    pub repo_id: String,
    pub path: String,
    pub created_at: i64,
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
) -> Result<Json<Vec<ShareLinkInfo>>, AppError> {
    let links = share_link::Entity::find()
        .filter(share_link::Column::CreatorId.eq(auth.user_id))
        .all(state.db.as_ref())
        .await?;

    let infos: Vec<ShareLinkInfo> = links
        .into_iter()
        .map(|l| ShareLinkInfo {
            token: l.token.clone(),
            link: format!("/f/{}/", l.token),
            repo_id: l.repo_id,
            path: l.path,
            created_at: l.created_at,
        })
        .collect();

    Ok(Json(infos))
}

pub async fn create_share_link(
    auth: AuthUser,
    State(state): State<Arc<AppState>>,
    Json(req): Json<CreateShareLinkRequest>,
) -> Result<Json<ShareLinkInfo>, AppError> {
    let token = generate_share_link_token();
    let now = chrono::Utc::now().timestamp();

    let password_hash = req
        .password
        .map(|p| crate::auth::password::hash_password_legacy(&p));

    let model = share_link::ActiveModel {
        id: sea_orm::NotSet,
        repo_id: sea_orm::Set(req.repo_id.clone()),
        creator_id: sea_orm::Set(auth.user_id),
        path: sea_orm::Set(req.path.clone()),
        token: sea_orm::Set(token.clone()),
        password: sea_orm::Set(password_hash),
        expires_at: sea_orm::Set(req.expires_at),
        created_at: sea_orm::Set(now),
    };
    share_link::Entity::insert(model)
        .exec(state.db.as_ref())
        .await?;

    Ok(Json(ShareLinkInfo {
        token: token.clone(),
        link: format!("/f/{}/", token),
        repo_id: req.repo_id,
        path: req.path,
        created_at: now,
    }))
}

pub async fn delete_share_link(
    auth: AuthUser,
    State(state): State<Arc<AppState>>,
    Path(token): Path<String>,
) -> Result<(), AppError> {
    share_link::Entity::delete_many()
        .filter(share_link::Column::Token.eq(&token))
        .filter(share_link::Column::CreatorId.eq(auth.user_id))
        .exec(state.db.as_ref())
        .await?;

    Ok(())
}

#[derive(Deserialize)]
pub struct BeshareRequest {
    pub share_type: String,
    pub user: String,
    pub permission: Option<String>,
}

/// `POST /api2/beshared-repos/{repo_id}/`
///
/// Shares a repo with another user by adding them as a repo member.
pub async fn beshare_repo(
    _auth: AuthUser,
    State(state): State<Arc<AppState>>,
    Path(repo_id): Path<String>,
    Json(req): Json<BeshareRequest>,
) -> Result<Json<serde_json::Value>, AppError> {
    if req.user.is_empty() {
        return Err(AppError::BadRequest("user email is required".into()));
    }

    // Find the target user
    let target_user = crate::entity::user::Entity::find()
        .filter(crate::entity::user::Column::Email.eq(&req.user))
        .one(state.db.as_ref())
        .await?
        .ok_or_else(|| AppError::BadRequest("user not found".into()))?;

    // Check if the membership already exists
    let existing = crate::entity::repo_member::Entity::find()
        .filter(crate::entity::repo_member::Column::RepoId.eq(&repo_id))
        .filter(crate::entity::repo_member::Column::UserId.eq(target_user.id))
        .one(state.db.as_ref())
        .await?;

    if existing.is_some() {
        return Ok(Json(
            serde_json::json!({"success": true, "already_shared": true}),
        ));
    }

    // Add repo member
    let now = chrono::Utc::now().timestamp();
    let perm = req.permission.unwrap_or_else(|| "rw".to_string());

    crate::entity::repo_member::Entity::insert(crate::entity::repo_member::ActiveModel {
        id: sea_orm::NotSet,
        repo_id: sea_orm::Set(repo_id),
        user_id: sea_orm::Set(target_user.id),
        permission: sea_orm::Set(perm),
        created_at: sea_orm::Set(now),
    })
    .exec(state.db.as_ref())
    .await?;

    Ok(Json(serde_json::json!({"success": true})))
}
