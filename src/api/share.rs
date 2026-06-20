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
use crate::entity::{repo, share_link};
use crate::error::AppError;
use crate::notification::events::FolderPermEvent;

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
    pub has_password: bool,
    pub expire_at: Option<i64>,
    pub s_type: String,
    pub view_cnt: i64,
    pub description: Option<String>,
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
            has_password: l.password.is_some(),
            expire_at: l.expires_at,
            s_type: l.s_type,
            view_cnt: l.view_cnt,
            description: l.description,
        })
        .collect();

    Ok(Json(infos))
}

pub async fn create_share_link(
    auth: AuthUser,
    State(state): State<Arc<AppState>>,
    Json(req): Json<CreateShareLinkRequest>,
) -> Result<Json<ShareLinkInfo>, AppError> {
    // Block share links for encrypted repos
    let repo_model = repo::Entity::find_by_id(&req.repo_id)
        .one(state.db.as_ref())
        .await?
        .ok_or_else(|| AppError::NotFound("repo not found".into()))?;
    if repo_model.encrypted != 0 {
        return Err(AppError::BadRequest(
            "cannot create share link for encrypted library".into(),
        ));
    }

    // Verify caller has read permission on the repo
    crate::storage::check_repo_read_permission(state.db.as_ref(), &req.repo_id, auth.user_id)
        .await?;

    // s_type defaults to 'f' (file). Full path-to-type resolution requires
    // walking the commit tree, which is done lazily at download time.
    let s_type = "f".to_string();

    let token = generate_share_link_token();
    let now = chrono::Utc::now().timestamp();

    let password_hash = req.password.as_ref().map(|p| {
        crate::auth::password::hash_password(p, state.config.auth.password_hash_iterations)
    });
    let has_password = req.password.is_some();

    let model = share_link::ActiveModel {
        id: sea_orm::NotSet,
        repo_id: sea_orm::Set(req.repo_id.clone()),
        creator_id: sea_orm::Set(auth.user_id),
        path: sea_orm::Set(req.path.clone()),
        token: sea_orm::Set(token.clone()),
        password: sea_orm::Set(password_hash),
        expires_at: sea_orm::Set(req.expires_at),
        created_at: sea_orm::Set(now),
        s_type: sea_orm::Set(s_type.clone()),
        view_cnt: sea_orm::Set(0i64),
        description: sea_orm::Set(None),
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
        has_password,
        expire_at: req.expires_at,
        s_type,
        view_cnt: 0,
        description: None,
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
    auth: AuthUser,
    State(state): State<Arc<AppState>>,
    Path(repo_id): Path<String>,
    Json(req): Json<BeshareRequest>,
) -> Result<Json<serde_json::Value>, AppError> {
    if req.user.is_empty() {
        return Err(AppError::BadRequest("user email is required".into()));
    }

    // Verify caller has write permission on the repo
    crate::storage::check_repo_write_permission(state.db.as_ref(), &repo_id, auth.user_id).await?;

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
        repo_id: sea_orm::Set(repo_id.clone()),
        user_id: sea_orm::Set(target_user.id),
        permission: sea_orm::Set(perm.clone()),
        created_at: sea_orm::Set(now),
    })
    .exec(state.db.as_ref())
    .await?;

    // Send WebSocket notification about the share change.
    if let Some(mgr) = &state.notification_manager {
        let event = FolderPermEvent {
            repo_id: repo_id.clone(),
            path: "/".to_string(),
            event_type: "user".to_string(),
            change_event: "add".to_string(),
            user: req.user.clone(),
            group: -1,
            perm,
        };
        mgr.notify(event).await;
    }

    Ok(Json(serde_json::json!({"success": true})))
}

// ── Share member management (permission modification) ──────────────

/// Response for a share member entry.
#[derive(Serialize)]
pub struct ShareMember {
    pub email: String,
    pub permission: String,
    pub created_at: i64,
}

/// Request for modifying or deleting a share.
#[derive(Deserialize)]
pub struct ModifyShareRequest {
    pub user: String,
    pub permission: Option<String>,
}

/// `GET /api2/beshared-repos/{repo_id}/` — list all users shared to this repo.
pub async fn list_share_members(
    auth: AuthUser,
    State(state): State<Arc<AppState>>,
    Path(repo_id): Path<String>,
) -> Result<Json<Vec<ShareMember>>, AppError> {
    // Only the repo owner can list share members.
    crate::storage::check_repo_write_permission(state.db.as_ref(), &repo_id, auth.user_id).await?;

    let members = crate::entity::repo_member::Entity::find()
        .filter(crate::entity::repo_member::Column::RepoId.eq(&repo_id))
        .all(state.db.as_ref())
        .await?;

    let mut result = Vec::new();
    for m in members {
        let user_record = crate::entity::user::Entity::find_by_id(m.user_id)
            .one(state.db.as_ref())
            .await?;
        if let Some(u) = user_record {
            result.push(ShareMember {
                email: u.email,
                permission: m.permission,
                created_at: m.created_at,
            });
        }
    }
    Ok(Json(result))
}

/// `PUT /api2/beshared-repos/{repo_id}/` — modify a user's share permission.
pub async fn modify_share_permission(
    auth: AuthUser,
    State(state): State<Arc<AppState>>,
    Path(repo_id): Path<String>,
    Json(req): Json<ModifyShareRequest>,
) -> Result<Json<serde_json::Value>, AppError> {
    if req.user.is_empty() {
        return Err(AppError::BadRequest("user email is required".into()));
    }
    let new_perm = req
        .permission
        .as_deref()
        .ok_or_else(|| AppError::BadRequest("permission is required".into()))?;
    if new_perm != "rw" && new_perm != "r" {
        return Err(AppError::BadRequest(
            "permission must be 'rw' or 'r'".into(),
        ));
    }

    // Only the repo owner can modify permissions.
    crate::storage::check_repo_write_permission(state.db.as_ref(), &repo_id, auth.user_id).await?;

    let target_user = crate::entity::user::Entity::find()
        .filter(crate::entity::user::Column::Email.eq(&req.user))
        .one(state.db.as_ref())
        .await?
        .ok_or_else(|| AppError::BadRequest("user not found".into()))?;

    use sea_orm::{ActiveModelTrait, Set};
    let member = crate::entity::repo_member::Entity::find()
        .filter(crate::entity::repo_member::Column::RepoId.eq(&repo_id))
        .filter(crate::entity::repo_member::Column::UserId.eq(target_user.id))
        .one(state.db.as_ref())
        .await?
        .ok_or_else(|| AppError::BadRequest("user is not a member of this repo".into()))?;

    let mut active: crate::entity::repo_member::ActiveModel = member.into();
    active.permission = Set(new_perm.to_string());
    active.update(state.db.as_ref()).await?;

    // Send WebSocket notification about the permission change.
    if let Some(mgr) = &state.notification_manager {
        let event = FolderPermEvent {
            repo_id: repo_id.clone(),
            path: "/".to_string(),
            event_type: "user".to_string(),
            change_event: "modify".to_string(),
            user: req.user.clone(),
            group: -1,
            perm: new_perm.to_string(),
        };
        mgr.notify(event).await;
    }

    Ok(Json(serde_json::json!({"success": true})))
}

/// `DELETE /api2/beshared-repos/{repo_id}/` — remove a user's share.
pub async fn delete_share(
    auth: AuthUser,
    State(state): State<Arc<AppState>>,
    Path(repo_id): Path<String>,
    Json(req): Json<ModifyShareRequest>,
) -> Result<Json<serde_json::Value>, AppError> {
    if req.user.is_empty() {
        return Err(AppError::BadRequest("user email is required".into()));
    }

    // Only the repo owner can delete shares.
    crate::storage::check_repo_write_permission(state.db.as_ref(), &repo_id, auth.user_id).await?;

    let target_user = crate::entity::user::Entity::find()
        .filter(crate::entity::user::Column::Email.eq(&req.user))
        .one(state.db.as_ref())
        .await?
        .ok_or_else(|| AppError::BadRequest("user not found".into()))?;

    crate::entity::repo_member::Entity::delete_many()
        .filter(crate::entity::repo_member::Column::RepoId.eq(&repo_id))
        .filter(crate::entity::repo_member::Column::UserId.eq(target_user.id))
        .exec(state.db.as_ref())
        .await?;

    // Send WebSocket notification about the share deletion.
    if let Some(mgr) = &state.notification_manager {
        let event = FolderPermEvent {
            repo_id: repo_id.clone(),
            path: "/".to_string(),
            event_type: "user".to_string(),
            change_event: "del".to_string(),
            user: req.user.clone(),
            group: -1,
            perm: String::new(),
        };
        mgr.notify(event).await;
    }

    Ok(Json(serde_json::json!({"success": true})))
}
