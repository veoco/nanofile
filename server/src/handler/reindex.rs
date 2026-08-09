use axum::{
    Json,
    extract::{Query, State},
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

use crate::AppState;
use crate::ReindexProgress;
use crate::middleware::auth::AuthUser;
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
    pub task_id: String,
}

#[derive(Deserialize)]
pub struct ReindexProgressQuery {
    pub task_id: String,
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
///
/// Runs as a background task so large repositories don't block the HTTP
/// response. The task id can be polled via `GET /api2/reindex-progress/`.
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
        .clone()
        .ok_or_else(|| AppError::BadRequest("full-text indexing is not enabled".into()))?;

    let task_id = uuid::Uuid::new_v4().to_string();
    {
        let mut map = state.reindex_tasks.lock().unwrap();
        map.insert(
            task_id.clone(),
            ReindexProgress {
                state: "running".to_string(),
                repo_id: req.repo_id.clone(),
                done_count: 0,
                total: 0,
                indexed: 0,
                skipped: 0,
                error: None,
            },
        );
    }

    let state_clone = state.clone();
    let tid = task_id.clone();
    let rid = req.repo_id.clone();
    let block_store = state.block_store.clone();

    tokio::spawn(async move {
        let progress_handle = state_clone.reindex_tasks.clone();
        let tid_inner = tid.clone();
        let on_progress = move |done: u64, total: u64| {
            if let Ok(mut map) = progress_handle.lock()
                && let Some(p) = map.get_mut(&tid_inner)
            {
                p.done_count = done;
                p.total = total;
            }
        };

        let result = state_clone
            .admin_service()
            .reindex(&indexer, &rid, &block_store, on_progress)
            .await;

        let mut map = state_clone.reindex_tasks.lock().unwrap();
        if let Some(p) = map.get_mut(&tid) {
            match result {
                Ok((indexed, skipped)) => {
                    p.state = "completed".to_string();
                    p.indexed = indexed;
                    p.skipped = skipped;
                }
                Err(e) => {
                    p.state = "failed".to_string();
                    p.error = Some(e.to_string());
                }
            }
        }
    });

    Ok(Json(ReindexResponse {
        status: "ok".to_string(),
        task_id,
    }))
}

/// GET /api2/reindex-progress/
///
/// Poll the progress of a background reindex task.
pub async fn reindex_progress(
    _auth: AuthUser,
    State(state): State<Arc<AppState>>,
    Query(q): Query<ReindexProgressQuery>,
) -> Result<Json<ReindexProgress>, AppError> {
    let map = state.reindex_tasks.lock().unwrap();
    let p = map
        .get(&q.task_id)
        .ok_or_else(|| AppError::NotFound("reindex task not found".into()))?;
    Ok(Json(p.clone()))
}
