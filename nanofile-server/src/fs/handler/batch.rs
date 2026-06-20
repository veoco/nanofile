use axum::{Json, extract::State};
use serde::Deserialize;
use std::sync::Arc;

use crate::AppState;
use crate::auth::middleware::AuthUser;
use crate::common::util::normalize_path;
use crate::error::AppError;
use crate::fs::service::fileops_service::FileOpsService;

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
    req: Json<BatchMoveRequest>,
) -> Result<Json<serde_json::Value>, AppError> {
    if req.src_repo_id != req.dst_repo_id {
        return Err(AppError::BadRequest("cross-repo move not supported".into()));
    }

    let repo_id = &req.src_repo_id;
    let db = state.db.as_ref();

    crate::storage::check_repo_write_permission(db, repo_id, auth.user_id).await?;

    if req.src_dirents.is_empty() {
        return Ok(Json(serde_json::json!({"success": true})));
    }

    let svc = FileOpsService::new(
        state.db.clone(),
        state.block_store.clone(),
        state.indexer.clone(),
    );

    let src_dir = normalize_path(&req.src_parent_dir);
    let dst_dir = normalize_path(&req.dst_parent_dir);

    svc.batch_move(
        repo_id,
        &src_dir,
        &dst_dir,
        &req.src_dirents,
        &auth.email,
        auth.user_id,
    )
    .await?;

    Ok(Json(serde_json::json!({"success": true})))
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
    Json(body): Json<SyncBatchCopyRequest>,
) -> Result<Json<serde_json::Value>, AppError> {
    if body.src_repo_id != body.dst_repo_id {
        return Err(AppError::BadRequest("cross-repo copy not supported".into()));
    }

    let repo_id = &body.src_repo_id;
    let db = state.db.as_ref();

    crate::storage::check_repo_write_permission(db, repo_id, auth.user_id).await?;

    let svc = FileOpsService::new(
        state.db.clone(),
        state.block_store.clone(),
        state.indexer.clone(),
    );

    let src_parent_dir = normalize_path(&body.src_parent_dir);
    let dst_parent_dir = normalize_path(&body.dst_parent_dir);

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

    Ok(Json(serde_json::json!({"success": true})))
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
    Json(body): Json<BatchDeleteRequest>,
) -> Result<Json<serde_json::Value>, AppError> {
    if body.dirents.is_empty() {
        return Ok(Json(serde_json::json!({"success": true})));
    }

    let db = state.db.as_ref();
    let repo_id = &body.repo_id;

    crate::storage::check_repo_write_permission(db, repo_id, auth.user_id).await?;

    let parent_dir = normalize_path(&body.parent_dir);

    let svc = FileOpsService::new(
        state.db.clone(),
        state.block_store.clone(),
        state.indexer.clone(),
    );

    svc.batch_delete(
        repo_id,
        &parent_dir,
        &body.dirents,
        &auth.email,
        auth.user_id,
    )
    .await?;

    Ok(Json(serde_json::json!({"success": true})))
}
