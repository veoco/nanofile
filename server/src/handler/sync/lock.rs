use axum::{
    Json, Router,
    extract::{Path, Query, State},
    http::StatusCode,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

use crate::AppState;
use crate::middleware::auth::SyncAuth;
use base::error::AppError;

#[derive(Deserialize)]
pub struct LockQuery {
    pub p: Option<String>,
}

#[derive(Serialize)]
pub struct LockResponse {
    pub success: bool,
}

/// Response entry for the batch locked-files endpoint.
#[derive(Serialize)]
pub struct LockedFileEntry {
    pub path: String,
    /// 1 = current user locked the file, 0 = locked by another user.
    pub by_me: i32,
}

/// Request entry for the batch locked-files endpoint.
#[derive(Deserialize)]
pub struct LockedFilesReq {
    pub repo_id: String,
    pub token: String,
    pub ts: i64,
}

/// Response entry for the batch locked-files endpoint.
#[derive(Serialize)]
pub struct LockedFilesRes {
    pub repo_id: String,
    pub ts: i64,
    pub locked_files: Vec<LockedFileEntry>,
}

/// `POST /seafhttp/repo/{repo_id}/lock-file?p=path`
pub async fn lock_file(
    _auth: SyncAuth,
    State(state): State<Arc<AppState>>,
    Path(repo_id): Path<String>,
    Query(query): Query<LockQuery>,
) -> Result<Json<LockResponse>, AppError> {
    let path = query
        .p
        .as_deref()
        .ok_or_else(|| AppError::BadRequest("path required".into()))?;

    state
        .file_service()
        .lock_file_sync(&repo_id, path, _auth.user_id)
        .await?;

    Ok(Json(LockResponse { success: true }))
}

/// `POST /seafhttp/repo/{repo_id}/unlock-file?p=path`
pub async fn unlock_file(
    _auth: SyncAuth,
    State(state): State<Arc<AppState>>,
    Path(repo_id): Path<String>,
    Query(query): Query<LockQuery>,
) -> Result<Json<LockResponse>, AppError> {
    let path = query
        .p
        .as_deref()
        .ok_or_else(|| AppError::BadRequest("path required".into()))?;

    state
        .file_service()
        .unlock_file_sync(&repo_id, path, _auth.user_id)
        .await?;

    Ok(Json(LockResponse { success: true }))
}

/// `POST /seafhttp/repo/locked-files`
pub async fn list_locked_files_post(
    State(state): State<Arc<AppState>>,
    body: String,
) -> Result<(StatusCode, Json<Vec<LockedFilesRes>>), AppError> {
    let requests: Vec<LockedFilesReq> = serde_json::from_str(&body)
        .map_err(|e| AppError::BadRequest(format!("invalid JSON: {}", e)))?;

    let svc = state.file_service();
    let mut results = Vec::new();
    for req in &requests {
        let (files, lock_ts) = svc
            .get_locked_files_for_repo(&req.repo_id, &req.token)
            .await?;
        results.push(LockedFilesRes {
            repo_id: req.repo_id.clone(),
            ts: lock_ts,
            locked_files: files
                .into_iter()
                .map(|(path, by_me)| LockedFileEntry { path, by_me })
                .collect(),
        });
    }

    Ok((StatusCode::OK, Json(results)))
}

pub fn lock_routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/{repo_id}/lock-file", axum::routing::put(lock_file))
        .route("/{repo_id}/unlock-file", axum::routing::put(unlock_file))
        .route("/locked-files", axum::routing::post(list_locked_files_post))
}
