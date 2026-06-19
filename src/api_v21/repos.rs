use axum::Json;
use axum::extract::{Path, State};
use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};
use serde::Serialize;
use std::sync::Arc;

use crate::AppState;
use crate::activity_log;
use crate::auth::middleware::AuthUser;
use crate::entity::{repo, repo_member, sync_token, user};
use crate::error::AppError;

#[derive(Serialize)]
pub struct V21RepoListResponse {
    pub repos: Vec<V21RepoInfo>,
}

#[derive(Serialize)]
pub struct V21RepoInfo {
    pub repo_id: String,
    pub repo_name: String,
    pub repo_desc: String,
    pub permission: String,
    pub encrypted: bool,
    #[serde(rename = "type")]
    pub type_: String,
    pub size: i64,
    pub last_modified: String,
    pub mtime: i64,
    pub owner_email: String,
    pub owner_name: String,
}

/// GET /api/v2.1/repos/
pub async fn list_repos_v21(
    auth: AuthUser,
    State(state): State<Arc<AppState>>,
) -> Result<Json<V21RepoListResponse>, AppError> {
    let memberships = repo_member::Entity::find()
        .filter(repo_member::Column::UserId.eq(auth.user_id))
        .all(state.db.as_ref())
        .await?;

    let mut repos_list = Vec::new();
    for m in &memberships {
        if let Some(r) = repo::Entity::find_by_id(&m.repo_id)
            .one(state.db.as_ref())
            .await?
        {
            let is_owner = r.owner_id == auth.user_id;
            let repo_type = if is_owner { "mine" } else { "shared" };
            let owner_email = if is_owner {
                auth.email.clone()
            } else {
                user::Entity::find_by_id(r.owner_id)
                    .one(state.db.as_ref())
                    .await?
                    .map(|u| u.email)
                    .unwrap_or_default()
            };

            repos_list.push(V21RepoInfo {
                repo_id: r.id,
                repo_name: r.name,
                repo_desc: r.description,
                permission: m.permission.clone(),
                encrypted: r.encrypted != 0,
                type_: repo_type.to_string(),
                size: r.size,
                last_modified: chrono::DateTime::from_timestamp(r.updated_at, 0)
                    .map(|d| d.to_rfc3339())
                    .unwrap_or_default(),
                mtime: r.updated_at,
                owner_email: owner_email.clone(),
                owner_name: owner_email,
            });
        }
    }

    Ok(Json(V21RepoListResponse { repos: repos_list }))
}

/// GET /api/v2.1/repos/{repo_id}/
pub async fn get_repo_v21(
    auth: AuthUser,
    State(state): State<Arc<AppState>>,
    Path(repo_id): Path<String>,
) -> Result<Json<V21RepoInfo>, AppError> {
    let membership = repo_member::Entity::find()
        .filter(repo_member::Column::RepoId.eq(&repo_id))
        .filter(repo_member::Column::UserId.eq(auth.user_id))
        .one(state.db.as_ref())
        .await?
        .ok_or_else(|| AppError::NotFound("repo not found".into()))?;

    let r = repo::Entity::find_by_id(&repo_id)
        .one(state.db.as_ref())
        .await?
        .ok_or_else(|| AppError::NotFound("repo not found".into()))?;

    let is_owner = r.owner_id == auth.user_id;
    let repo_type = if is_owner { "mine" } else { "shared" };
    let owner_email = if is_owner {
        auth.email.clone()
    } else {
        user::Entity::find_by_id(r.owner_id)
            .one(state.db.as_ref())
            .await?
            .map(|u| u.email)
            .unwrap_or_default()
    };

    Ok(Json(V21RepoInfo {
        repo_id: r.id,
        repo_name: r.name,
        repo_desc: r.description,
        permission: membership.permission,
        encrypted: r.encrypted != 0,
        type_: repo_type.to_string(),
        size: r.size,
        last_modified: chrono::DateTime::from_timestamp(r.updated_at, 0)
            .map(|d| d.to_rfc3339())
            .unwrap_or_default(),
        mtime: r.updated_at,
        owner_email: owner_email.clone(),
        owner_name: owner_email,
    }))
}
pub async fn delete_repo_v21(
    auth: AuthUser,
    State(state): State<Arc<AppState>>,
    Path(repo_id): Path<String>,
) -> Result<Json<serde_json::Value>, AppError> {
    let db = state.db.as_ref();

    // Load repo
    let r = repo::Entity::find_by_id(&repo_id)
        .one(db)
        .await?
        .ok_or_else(|| AppError::NotFound("repo not found".into()))?;

    // Only owner can delete
    if r.owner_id != auth.user_id {
        return Err(AppError::Forbidden);
    }

    // --- REPO TRASH: Record deleted repo before cascade-delete ---
    if let Err(e) = crate::storage::trash::TrashService::add_deleted_repo(
        db,
        &repo_id,
        &r.name,
        r.head_commit_id.as_deref(),
        r.owner_id,
        r.size,
    )
    .await
    {
        tracing::warn!("Failed to record deleted repo in trash: {e}");
    }
    // --- END REPO TRASH ---

    // Log repo deletion activity BEFORE deleting the repo (FK constraint
    // prevents inserting activity with a non-existent repo_id).
    activity_log::log_activity(
        db,
        &repo_id,
        "delete",
        "repo",
        "/",
        auth.user_id,
        None,
        None,
        None,
        None,
    )
    .await;

    // Cascade-delete related records
    repo_member::Entity::delete_many()
        .filter(repo_member::Column::RepoId.eq(&repo_id))
        .exec(db)
        .await?;

    sync_token::Entity::delete_many()
        .filter(sync_token::Column::RepoId.eq(&repo_id))
        .exec(db)
        .await?;

    // Delete the repo itself
    repo::Entity::delete_by_id(&repo_id).exec(db).await?;

    Ok(Json(serde_json::Value::String("success".to_string())))
}
