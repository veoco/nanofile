use axum::{
    Json,
    extract::{Query, State},
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

use crate::AppState;
use crate::middleware::auth::AuthUser;
use crate::tasks::compat::reindex_progress as project_reindex_progress;
use crate::tasks::run::{RunId, Viewer};
use crate::tasks::spec::JobKey;
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

/// POST /api2/index-file-text/ — update one file's full-text index with custom text.
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

    // The repo id arrives in the JSON body, so the path-based key guard never
    // sees it: enforce the key's library scope before touching the index.
    auth.ensure_repo_allowed(&req.repo_id, true)?;

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
/// Rebuild a repository's full-text index as a background job polled via
/// `GET /api2/reindex-progress/`. Owner or admin only; a per-repo dedup lock and a
/// per-user rate limiter cap the frequency.
pub async fn reindex(
    auth: AuthUser,
    State(state): State<Arc<AppState>>,
    Json(req): Json<ReindexRequest>,
) -> Result<Json<ReindexResponse>, AppError> {
    let svc = state.admin_service();

    // The repo id arrives in the JSON body; see `index_file_text`.
    auth.ensure_repo_allowed(&req.repo_id, true)?;

    svc.check_repo_admin(&req.repo_id, auth.user_id).await?;

    if state.indexer.is_none() {
        return Err(AppError::BadRequest(
            "full-text indexing is not enabled".into(),
        ));
    }

    let rl_key = format!("reindex:{}", auth.user_id);
    if state.auth_limiters.reindex.is_limited(&rl_key) {
        return Err(AppError::TooManyRequests);
    }

    // Per-repo dedup: only one reindex at a time per repo, enforced by the task
    // system from the job's declared dedup key — no second map to keep in step.
    let task_id = state
        .tasks
        .submit_with_details(
            JobKey::Reindex,
            Some(auth.user_id),
            serde_json::json!({ "repo_id": req.repo_id }),
            format!("Reindex \"{}\"", req.repo_id),
            None,
            vec![("repo_id", serde_json::json!(req.repo_id.clone()))],
        )
        .await?;

    // Dedup passed — consume a rate-limit slot.
    state.auth_limiters.reindex.record_attempt(&rl_key);

    Ok(Json(ReindexResponse {
        status: "ok".to_string(),
        task_id: task_id.as_str().to_string(),
    }))
}

/// GET /api2/reindex-progress/ — poll a background reindex task; only its creator or
/// an admin may query. Completed tasks are purged after an hour by the run store.
pub async fn reindex_progress(
    auth: AuthUser,
    State(state): State<Arc<AppState>>,
    Query(q): Query<ReindexProgressQuery>,
) -> Result<Json<serde_json::Value>, AppError> {
    // The viewer check is part of the lookup, so a run that exists but belongs
    // to somebody else is reported exactly like one that does not.
    let viewer = Viewer::User {
        id: auth.user_id,
        is_admin: state
            .repos
            .user
            .find_by_id(auth.user_id)
            .await?
            .is_some_and(|user| user.is_admin),
    };
    let run = state
        .tasks
        .store()
        .get_for(&RunId::from_client(q.task_id), viewer)
        .ok_or_else(|| AppError::NotFound("reindex task not found".into()))?;

    Ok(Json(project_reindex_progress(&run)))
}
