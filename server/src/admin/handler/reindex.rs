use axum::{Json, extract::State};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

use crate::AppState;
use crate::auth::middleware::AuthUser;
use base::error::AppError;

#[derive(Deserialize)]
pub struct ReindexRequest {
    pub repo_id: String,
}

#[derive(Deserialize)]
pub struct IndexFileTextRequest {
    pub repo_id: String,
    pub path: String,
    pub text: String,
}

#[derive(Serialize)]
pub struct ReindexResponse {
    pub status: String,
    pub indexed: u64,
    pub skipped: u64,
}

#[derive(Serialize)]
pub struct IndexFileTextResponse {
    pub status: String,
}

/// POST /api2/index-file-text/
///
/// Update the full-text search index for a specific file with custom text.
/// Handler is thin: auth → validate → call service → format response.
pub async fn index_file_text(
    auth: AuthUser,
    State(state): State<Arc<AppState>>,
    Json(req): Json<IndexFileTextRequest>,
) -> Result<Json<IndexFileTextResponse>, AppError> {
    if req.path.is_empty() {
        return Err(AppError::BadRequest("path is required".into()));
    }
    if req.text.is_empty() {
        return Err(AppError::BadRequest("text is required".into()));
    }

    let svc = state.admin_service();

    // Verify access to the repo.
    svc.check_repo_access(&req.repo_id, auth.user_id).await?;

    let indexer = state
        .indexer
        .as_ref()
        .ok_or_else(|| AppError::BadRequest("full-text indexing is not enabled".into()))?;

    svc.index_file_text(indexer, &req.repo_id, &req.path, &req.text)?;

    Ok(Json(IndexFileTextResponse {
        status: "ok".to_string(),
    }))
}

/// POST /api2/reindex/
///
/// Rebuild the full-text search index for all files in a repository.
pub async fn reindex(
    auth: AuthUser,
    State(state): State<Arc<AppState>>,
    Json(req): Json<ReindexRequest>,
) -> Result<Json<ReindexResponse>, AppError> {
    let svc = state.admin_service();

    // Verify the user has access to this repo.
    svc.check_repo_access(&req.repo_id, auth.user_id).await?;

    let indexer = state
        .indexer
        .as_ref()
        .ok_or_else(|| AppError::BadRequest("full-text indexing is not enabled".into()))?;

    let (indexed, skipped) = svc
        .reindex(indexer, &req.repo_id, &state.block_store)
        .await?;

    Ok(Json(ReindexResponse {
        status: "ok".to_string(),
        indexed,
        skipped,
    }))
}
