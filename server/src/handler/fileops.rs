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
use crate::middleware::repo_extractor::RepoPathWrite;
use crate::repository::Repositories;
use crate::service::fs::fileops::{self as fops_svc};
use base::error::AppError;
use base::sanitize::safe_normalize_path;

/// Get the directory listing for a path (reloaddir=true support).
#[derive(Serialize)]
pub struct DirEntry {
    pub id: String,
    #[serde(rename = "type")]
    pub entry_type: String,
    pub name: String,
    pub size: i64,
    pub mtime: i64,
    pub permission: String,
}

async fn list_dir_from_fs_tree(
    repos: &Repositories,
    repo_id: &str,
    path: &str,
) -> Result<(String, Vec<DirEntry>), AppError> {
    let repo_record = repos
        .repo
        .find_by_id(repo_id)
        .await?
        .ok_or_else(|| AppError::NotFound("Repository not found".into()))?;

    let head_commit_id = match repo_record.head_commit_id {
        Some(id) => id,
        None => return Ok((String::new(), vec![])),
    };

    let head = repos
        .commit
        .find_by_repo_and_commit_id(repo_id, &head_commit_id)
        .await?
        .ok_or_else(|| AppError::NotFound("Head commit not found".into()))?;

    let dir_id = crate::fs::core::resolve_fs_id(repos, repo_id, &head.root_id, path)
        .await
        .map_err(|e| AppError::internal(format!("resolve_fs_id failed: {e}")))?;

    let dir_data = crate::fs::core::read_fs_dir_data(repos, repo_id, &dir_id)
        .await
        .map_err(|e| AppError::internal(format!("read fs_dir_data failed: {e}")))?;

    Ok((
        dir_id,
        dir_data
            .dirents
            .into_iter()
            .map(|d| DirEntry {
                id: d.id,
                entry_type: if d.mode & infra::serialization::S_IFDIR != 0 {
                    "dir".to_string()
                } else {
                    "file".to_string()
                },
                name: d.name,
                size: d.size,
                mtime: d.mtime,
                permission: "rw".to_string(),
            })
            .collect(),
    ))
}

// ─── Query / Request types ───────────────────────────────────

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

#[derive(Serialize)]
pub struct CopyMoveWithDirResult {
    pub repo_id: String,
    pub parent_dir: String,
    pub obj_name: String,
    pub dir_listing: Option<Vec<DirEntry>>,
}

// ─── Routes ──────────────────────────────────────────────────

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

// ─── Request body parsing ─────────────────────────────────────

async fn parse_form_body(
    req: Request<axum::body::Body>,
) -> Result<HashMap<String, String>, AppError> {
    let (_, body) = req.into_parts();
    let bytes = axum::body::to_bytes(body, usize::MAX)
        .await
        .map_err(|e| AppError::Internal(e.to_string()))?;
    serde_urlencoded::from_bytes(&bytes)
        .map_err(|_| AppError::BadRequest("invalid form data".into()))
}

// ─── Batch Delete ────────────────────────────────────────────

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

    // Handle reloaddir=true
    if query.reloaddir.as_deref() == Some("true") {
        let (_, entries) = list_dir_from_fs_tree(&state.repos, repo_id, &parent_dir).await?;
        return Ok(Json(json!({"dir_listing": entries})).into_response());
    }

    Ok(StatusCode::OK.into_response())
}

// ─── Batch Copy ──────────────────────────────────────────────

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

    // Convert results to response format
    let json_results: Vec<CopyMoveResult> = results
        .into_iter()
        .map(|r| CopyMoveResult {
            repo_id: r.repo_id,
            parent_dir: r.parent_dir,
            obj_name: r.obj_name,
        })
        .collect();

    if query.reloaddir.as_deref() == Some("true") {
        let (_, entries) = list_dir_from_fs_tree(&state.repos, repo_id, &dst_dir).await?;
        return Ok(Json(json!({
            "results": json_results,
            "dir_listing": entries,
        }))
        .into_response());
    }

    Ok(Json(json!(json_results)).into_response())
}

// ─── Batch Move ──────────────────────────────────────────────

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

    if query.reloaddir.as_deref() == Some("true") {
        let (_, entries) = list_dir_from_fs_tree(&state.repos, repo_id, &dst_dir).await?;
        return Ok(Json(json!({
            "results": json_results,
            "dir_listing": entries,
        }))
        .into_response());
    }

    Ok(Json(json!(json_results)).into_response())
}
