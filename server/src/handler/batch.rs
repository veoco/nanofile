use axum::{Json, extract::State};
use serde::Deserialize;
use std::sync::Arc;

use crate::AppState;
use crate::handler::{ensure_same_batch_repo, normalize_batch_dir, ok_json};
use crate::middleware::auth::AuthUser;
use base::error::AppError;

#[derive(Deserialize)]
pub struct BatchMoveRequest {
    pub src_repo_id: String,
    pub src_parent_dir: String,
    pub src_dirents: Vec<String>,
    pub dst_repo_id: String,
    pub dst_parent_dir: String,
}

pub async fn batch_move_items(
    auth: AuthUser,
    State(state): State<Arc<AppState>>,
    mut req: Json<BatchMoveRequest>,
) -> Result<Json<serde_json::Value>, AppError> {
    ensure_same_batch_repo(&req.src_repo_id, &req.dst_repo_id, "move")?;

    crate::handler::sanitize_dirent_list(&mut req.src_dirents)?;

    let repo_id = &req.src_repo_id;

    crate::domain::permission::check_repo_write_permission(
        state.repos.member.as_ref(),
        repo_id,
        auth.user_id,
    )
    .await?;

    // The repo id comes from the body, so the path-based key guard never sees it.
    auth.ensure_repo_allowed(repo_id, true)?;

    if req.src_dirents.is_empty() {
        return Ok(ok_json());
    }

    let svc = state.fileops_service();

    let src_dir = normalize_batch_dir(&req.src_parent_dir, "source path")?;
    let dst_dir = normalize_batch_dir(&req.dst_parent_dir, "destination path")?;

    svc.batch_move(
        repo_id,
        &src_dir,
        &dst_dir,
        &req.src_dirents,
        &auth.email,
        auth.user_id,
    )
    .await?;

    Ok(ok_json())
}

#[derive(Deserialize)]
pub struct SyncBatchCopyRequest {
    pub src_repo_id: String,
    pub src_parent_dir: String,
    pub src_dirents: Vec<String>,
    pub dst_repo_id: String,
    pub dst_parent_dir: String,
}

pub async fn sync_batch_copy_item(
    auth: AuthUser,
    State(state): State<Arc<AppState>>,
    mut body: Json<SyncBatchCopyRequest>,
) -> Result<Json<serde_json::Value>, AppError> {
    ensure_same_batch_repo(&body.src_repo_id, &body.dst_repo_id, "copy")?;

    crate::handler::sanitize_dirent_list(&mut body.src_dirents)?;

    let repo_id = &body.src_repo_id;

    crate::domain::permission::check_repo_write_permission(
        state.repos.member.as_ref(),
        repo_id,
        auth.user_id,
    )
    .await?;

    // The repo id comes from the body, so the path-based key guard never sees it.
    auth.ensure_repo_allowed(repo_id, true)?;

    let svc = state.fileops_service();

    let src_parent_dir = normalize_batch_dir(&body.src_parent_dir, "source path")?;
    let dst_parent_dir = normalize_batch_dir(&body.dst_parent_dir, "destination path")?;

    let _results = svc
        .batch_copy(
            repo_id,
            &src_parent_dir,
            &dst_parent_dir,
            &body.src_dirents,
            &auth.email,
            auth.user_id,
        )
        .await?;

    Ok(ok_json())
}

#[derive(Deserialize)]
pub struct BatchDeleteRequest {
    pub repo_id: String,
    pub parent_dir: String,
    pub dirents: Vec<String>,
}

pub async fn batch_delete_item(
    auth: AuthUser,
    State(state): State<Arc<AppState>>,
    mut body: Json<BatchDeleteRequest>,
) -> Result<Json<serde_json::Value>, AppError> {
    crate::handler::sanitize_dirent_list(&mut body.dirents)?;

    if body.dirents.is_empty() {
        return Ok(ok_json());
    }

    let repo_id = &body.repo_id;

    crate::domain::permission::check_repo_write_permission(
        state.repos.member.as_ref(),
        repo_id,
        auth.user_id,
    )
    .await?;

    // The repo id comes from the body, so the path-based key guard never sees it.
    auth.ensure_repo_allowed(repo_id, true)?;

    let parent_dir = normalize_batch_dir(&body.parent_dir, "path")?;

    let svc = state.fileops_service();

    svc.batch_delete(
        repo_id,
        &parent_dir,
        &body.dirents,
        &auth.email,
        auth.user_id,
    )
    .await?;

    Ok(ok_json())
}
