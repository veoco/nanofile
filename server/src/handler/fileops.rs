use axum::{
    Json, Router,
    extract::{Query, State},
    http::{Request, StatusCode},
    response::{IntoResponse, Response},
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::HashMap;
use std::sync::Arc;

use crate::AppState;
use crate::handler::{MAX_SMALL_BODY_BYTES, read_body_limited};
use crate::middleware::repo_extractor::RepoPathWrite;
use crate::service::fs::fileops::{self as fops_svc};
use base::error::AppError;
use base::sanitize::safe_normalize_path;

#[derive(Deserialize)]
pub struct FileOpsQuery {
    pub p: Option<String>,
    pub reloaddir: Option<String>,
}

#[derive(Serialize)]
pub struct CopyMoveResult {
    pub repo_id: String,
    pub parent_dir: String,
    pub obj_name: String,
}

pub fn fileops_routes() -> Router<Arc<AppState>> {
    Router::new()
        .route(
            "/{repo_id}/fileops/delete/",
            axum::routing::post(batch_delete_handler),
        )
        .route(
            "/{repo_id}/fileops/copy/",
            axum::routing::post(batch_copy_handler),
        )
        .route(
            "/{repo_id}/fileops/move/",
            axum::routing::post(batch_move_handler),
        )
}

async fn parse_form_body(
    req: Request<axum::body::Body>,
) -> Result<HashMap<String, String>, AppError> {
    let (_, body) = req.into_parts();
    let bytes = read_body_limited(body, MAX_SMALL_BODY_BYTES).await?;
    serde_urlencoded::from_bytes(&bytes)
        .map_err(|_| AppError::BadRequest("invalid form data".into()))
}

pub async fn batch_delete_handler(
    access: RepoPathWrite,
    State(state): State<Arc<AppState>>,
    Query(query): Query<FileOpsQuery>,
    req: Request<axum::body::Body>,
) -> Result<Response, AppError> {
    let repo_id = &access.repo_id;

    let form = parse_form_body(req).await?;
    let file_names_str = form.get("file_names").map(|s| s.as_str()).unwrap_or("");
    let file_names = fops_svc::parse_file_names(file_names_str);

    if file_names.is_empty() {
        return Ok(Json(json!({})).into_response());
    }

    let parent_dir = safe_normalize_path(query.p.as_deref().unwrap_or("/"))
        .map_err(|e| AppError::BadRequest(format!("Invalid path: {e}")))?;

    let svc = state.fileops_service();
    svc.batch_delete(
        repo_id,
        &parent_dir,
        &file_names,
        &access.user.email,
        access.user.user_id,
    )
    .await?;

    // reloaddir=true: seahub's `reloaddir_if_necessary` answers with the parent
    // directory's dirents as a **bare** array; the iOS client's `SeafDir.handleData:`
    // rejects any other wrapper, so `{"dir_listing": …}` left its list stale. The
    // listing (and its `oid`/`dir_perm` headers) comes from the one directory
    // service the other reloaddir endpoints use.
    if crate::handler::dir::is_true(query.reloaddir.as_deref()) {
        return crate::handler::dir::reload_dir_response(&state, repo_id, &parent_dir).await;
    }

    Ok(StatusCode::OK.into_response())
}

pub async fn batch_copy_handler(
    access: RepoPathWrite,
    State(state): State<Arc<AppState>>,
    Query(query): Query<FileOpsQuery>,
    req: Request<axum::body::Body>,
) -> Result<Response, AppError> {
    let repo_id = &access.repo_id;

    let form = parse_form_body(req).await?;
    let file_names_str = form.get("file_names").map(|s| s.as_str()).unwrap_or("");
    let file_names = fops_svc::parse_file_names(file_names_str);

    if file_names.is_empty() {
        return Ok(Json(json!([])).into_response());
    }

    let dst_repo = form
        .get("dst_repo")
        .ok_or_else(|| AppError::BadRequest("dst_repo required".into()))?;
    let dst_dir = safe_normalize_path(form.get("dst_dir").map(|s| s.as_str()).unwrap_or("/"))
        .map_err(|e| AppError::BadRequest(format!("Invalid destination path: {e}")))?;

    if *dst_repo != *repo_id {
        return Err(AppError::BadRequest("cross-repo copy not supported".into()));
    }

    let src_parent_dir = safe_normalize_path(query.p.as_deref().unwrap_or("/"))
        .map_err(|e| AppError::BadRequest(format!("Invalid source path: {e}")))?;

    let svc = state.fileops_service();
    let results = svc
        .batch_copy(
            repo_id,
            &src_parent_dir,
            &dst_dir,
            &file_names,
            &access.user.email,
            access.user.user_id,
        )
        .await?;

    let json_results: Vec<CopyMoveResult> = results
        .into_iter()
        .map(|r| CopyMoveResult {
            repo_id: r.repo_id,
            parent_dir: r.parent_dir,
            obj_name: r.obj_name,
        })
        .collect();

    // seahub reloads the **source** directory (`parent_dir = ?p`) after a copy.
    if crate::handler::dir::is_true(query.reloaddir.as_deref()) {
        return crate::handler::dir::reload_dir_response(&state, repo_id, &src_parent_dir).await;
    }

    Ok(Json(json!(json_results)).into_response())
}

pub async fn batch_move_handler(
    access: RepoPathWrite,
    State(state): State<Arc<AppState>>,
    Query(query): Query<FileOpsQuery>,
    req: Request<axum::body::Body>,
) -> Result<Response, AppError> {
    let repo_id = &access.repo_id;

    let form = parse_form_body(req).await?;
    let file_names_str = form.get("file_names").map(|s| s.as_str()).unwrap_or("");
    let file_names = fops_svc::parse_file_names(file_names_str);

    if file_names.is_empty() {
        return Ok(Json(json!([])).into_response());
    }

    let dst_repo = form
        .get("dst_repo")
        .ok_or_else(|| AppError::BadRequest("dst_repo required".into()))?;
    let dst_dir = safe_normalize_path(form.get("dst_dir").map(|s| s.as_str()).unwrap_or("/"))
        .map_err(|e| AppError::BadRequest(format!("Invalid destination path: {e}")))?;

    if *dst_repo != *repo_id {
        return Err(AppError::BadRequest("cross-repo move not supported".into()));
    }

    let src_parent_dir = safe_normalize_path(query.p.as_deref().unwrap_or("/"))
        .map_err(|e| AppError::BadRequest(format!("Invalid source path: {e}")))?;

    let svc = state.fileops_service();
    let results = svc
        .batch_move(
            repo_id,
            &src_parent_dir,
            &dst_dir,
            &file_names,
            &access.user.email,
            access.user.user_id,
        )
        .await?;

    let json_results: Vec<CopyMoveResult> = results
        .into_iter()
        .map(|r| CopyMoveResult {
            repo_id: r.repo_id,
            parent_dir: r.parent_dir,
            obj_name: r.obj_name,
        })
        .collect();

    // seahub reloads the **source** directory (`parent_dir = ?p`) after a move.
    if crate::handler::dir::is_true(query.reloaddir.as_deref()) {
        return crate::handler::dir::reload_dir_response(&state, repo_id, &src_parent_dir).await;
    }

    Ok(Json(json!(json_results)).into_response())
}
