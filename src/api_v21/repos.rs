use axum::{Json, extract::Path, extract::State, http::StatusCode};
use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};
use serde::Serialize;
use std::sync::Arc;

use crate::AppState;
use crate::auth::middleware::AuthUser;
use crate::entity::{repo, repo_member, sync_token};
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
    pub type_: String,
    pub size: i64,
    pub root: String,
    pub head_commit_id: String,
    pub version: i32,
    pub last_modified: i64,
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
            repos_list.push(V21RepoInfo {
                repo_id: r.id.clone(),
                repo_name: r.name,
                repo_desc: r.description,
                permission: m.permission.clone(),
                encrypted: r.encrypted != 0,
                type_: "repo".to_string(),
                size: r.size,
                root: r.head_commit_id.clone().unwrap_or_default(),
                head_commit_id: r.head_commit_id.unwrap_or_default(),
                version: r.repo_version,
                last_modified: r.updated_at,
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

    Ok(Json(V21RepoInfo {
        repo_id: r.id,
        repo_name: r.name,
        repo_desc: r.description,
        permission: membership.permission,
        encrypted: r.encrypted != 0,
        type_: "repo".to_string(),
        size: r.size,
        root: r.head_commit_id.clone().unwrap_or_default(),
        head_commit_id: r.head_commit_id.unwrap_or_default(),
        version: r.repo_version,
        last_modified: r.updated_at,
    }))
}

/// DELETE /api/v2.1/repos/{repo_id}/
pub async fn delete_repo_v21(
    auth: AuthUser,
    State(state): State<Arc<AppState>>,
    Path(repo_id): Path<String>,
) -> Result<StatusCode, AppError> {
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

    Ok(StatusCode::OK)
}
