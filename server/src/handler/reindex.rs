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

    // Only repo owner or admin may modify the index.
    svc.check_repo_admin(&req.repo_id, auth.user_id).await?;

    let indexer = state
        .indexer
        .as_ref()
        .ok_or_else(|| AppError::BadRequest("full-text indexing is not enabled".into()))?;

    svc.index_file_text(indexer, &req.repo_id, &req.path, &req.text)
        .await?;

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
///
/// Only the repo owner or a server admin may trigger a reindex. A per-repo
/// dedup lock prevents concurrent reindex of the same repo, and a per-user
/// rate limiter caps the frequency.
pub async fn reindex(
    auth: AuthUser,
    State(state): State<Arc<AppState>>,
    Json(req): Json<ReindexRequest>,
) -> Result<Json<ReindexResponse>, AppError> {
    let svc = state.admin_service();

    // Only owner or admin may trigger a full reindex.
    svc.check_repo_admin(&req.repo_id, auth.user_id).await?;

    let indexer = state
        .indexer
        .clone()
        .ok_or_else(|| AppError::BadRequest("full-text indexing is not enabled".into()))?;

    // Rate-limit per user to prevent abuse.
    let rl_key = format!("reindex:{}", auth.user_id);
    if state.auth_limiters.reindex.is_limited(&rl_key) {
        return Err(AppError::TooManyRequests);
    }

    // Per-repo dedup: only one reindex at a time per repo.
    {
        let mut running = state
            .reindex_running
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if running.contains_key(&req.repo_id) {
            return Err(AppError::Conflict(
                "reindex already in progress for this repo".into(),
            ));
        }
        running.insert(req.repo_id.clone(), ());
    }

    // Dedup passed — consume a rate-limit slot.
    state.auth_limiters.reindex.record_attempt(&rl_key);

    let task_id = uuid::Uuid::new_v4().to_string();
    {
        let mut map = state
            .reindex_tasks
            .lock()
            .unwrap_or_else(|e| e.into_inner());
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
                finished_at: None,
                creator_id: auth.user_id,
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

        let now = chrono::Utc::now().timestamp();
        let mut map = state_clone
            .reindex_tasks
            .lock()
            .unwrap_or_else(|e| e.into_inner());
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
            p.finished_at = Some(now);
        }
        drop(map);

        // Release the per-repo dedup lock.
        let mut running = state_clone
            .reindex_running
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        running.remove(&rid);
    });

    Ok(Json(ReindexResponse {
        status: "ok".to_string(),
        task_id,
    }))
}

/// GET /api2/reindex-progress/
///
/// Poll the progress of a background reindex task. Only the task creator
/// or a server admin may query progress. Completed tasks are purged after
/// one hour to bound memory usage.
pub async fn reindex_progress(
    auth: AuthUser,
    State(state): State<Arc<AppState>>,
    Query(q): Query<ReindexProgressQuery>,
) -> Result<Json<ReindexProgress>, AppError> {
    let now = chrono::Utc::now().timestamp();

    // Lock, do TTL cleanup, fetch the progress entry, then drop the guard
    // before any async DB query so the future stays Send.
    let progress = {
        let mut map = state
            .reindex_tasks
            .lock()
            .unwrap_or_else(|e| e.into_inner());

        // TTL cleanup: drop completed/failed tasks older than 1 hour.
        map.retain(|_, p| !matches!(p.finished_at, Some(ts) if now - ts > 3600));

        map.get(&q.task_id)
            .cloned()
            .ok_or_else(|| AppError::NotFound("reindex task not found".into()))?
    };

    // Only the creator or an admin may view progress.
    if progress.creator_id != auth.user_id {
        let user = state
            .repos
            .user
            .find_by_id(auth.user_id)
            .await?
            .ok_or(AppError::Forbidden)?;
        if !user.is_admin {
            return Err(AppError::Forbidden);
        }
    }

    Ok(Json(progress))
}
