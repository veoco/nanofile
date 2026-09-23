//! Async batch copy/move, and the progress endpoint the desktop client polls.
//!
//! These handlers are compatibility surface: the web UI uses the *synchronous*
//! batch endpoints, so the only consumer is the official client. The handlers
//! are therefore thin — validate, submit, answer with a task id — and the
//! lifecycle belongs entirely to the task system.

use axum::{
    Json,
    extract::{Query, State},
};
use serde::Deserialize;
use std::sync::Arc;

use crate::AppState;
use crate::middleware::auth::AuthUser;
use crate::tasks::compat::copy_move_progress;
use crate::tasks::run::Viewer;
use crate::tasks::spec::JobKey;
use base::error::AppError;
use base::sanitize::safe_normalize_path;

pub async fn async_batch_copy_item(
    auth: AuthUser,
    State(state): State<Arc<AppState>>,
    Json(mut body): Json<super::batch::SyncBatchCopyRequest>,
) -> Result<Json<serde_json::Value>, AppError> {
    crate::domain::permission::check_repo_write_permission(
        state.repos.member.as_ref(),
        &body.src_repo_id,
        auth.user_id,
    )
    .await?;

    // The library id comes from the request body, so the path-based key guard
    // never sees it.
    auth.ensure_repo_allowed(&body.src_repo_id, true)?;

    crate::handler::sanitize_dirent_list(&mut body.src_dirents)?;
    // The description below indexes `src_dirents[0]`, so an empty list must be
    // rejected here rather than panicking inside the handler.
    if body.src_dirents.is_empty() {
        return Err(AppError::BadRequest("no dirents specified".into()));
    }

    let src_dir = safe_normalize_path(&body.src_parent_dir)
        .map_err(|e| AppError::BadRequest(format!("Invalid source path: {e}")))?;
    let dst_dir = safe_normalize_path(&body.dst_parent_dir)
        .map_err(|e| AppError::BadRequest(format!("Invalid destination path: {e}")))?;

    let description = describe("Copy", &body.src_dirents);
    let total = body.src_dirents.len() as u64;
    let task_id = state
        .tasks
        .submit(
            JobKey::Copy,
            Some(auth.user_id),
            serde_json::json!({
                "repo_id": body.src_repo_id,
                "src_dir": src_dir,
                "dst_dir": dst_dir,
                "file_names": body.src_dirents,
                "email": auth.email,
            }),
            description,
            Some(total),
        )
        .await?;

    Ok(Json(serde_json::json!({"task_id": task_id.as_str()})))
}

pub async fn async_batch_move_item(
    auth: AuthUser,
    State(state): State<Arc<AppState>>,
    Json(mut body): Json<super::batch::BatchMoveRequest>,
) -> Result<Json<serde_json::Value>, AppError> {
    crate::domain::permission::check_repo_write_permission(
        state.repos.member.as_ref(),
        &body.src_repo_id,
        auth.user_id,
    )
    .await?;

    auth.ensure_repo_allowed(&body.src_repo_id, true)?;

    crate::handler::sanitize_dirent_list(&mut body.src_dirents)?;
    if body.src_dirents.is_empty() {
        return Err(AppError::BadRequest("no dirents specified".into()));
    }

    let src_dir = safe_normalize_path(&body.src_parent_dir)
        .map_err(|e| AppError::BadRequest(format!("Invalid source path: {e}")))?;
    let dst_dir = safe_normalize_path(&body.dst_parent_dir)
        .map_err(|e| AppError::BadRequest(format!("Invalid destination path: {e}")))?;

    let description = describe("Move", &body.src_dirents);
    let total = body.src_dirents.len() as u64;
    let task_id = state
        .tasks
        .submit(
            JobKey::Move,
            Some(auth.user_id),
            serde_json::json!({
                "repo_id": body.src_repo_id,
                "src_dir": src_dir,
                "dst_dir": dst_dir,
                "file_names": body.src_dirents,
                "email": auth.email,
            }),
            description,
            Some(total),
        )
        .await?;

    Ok(Json(serde_json::json!({"task_id": task_id.as_str()})))
}

/// `POST /api/v2.1/copy-move-task/` — the client's combined entry point.
#[derive(Deserialize)]
pub struct CopyMoveTaskRequest {
    pub src_repo_id: String,
    pub src_parent_dir: String,
    pub src_dirents: Vec<String>,
    pub dst_repo_id: String,
    pub dst_parent_dir: String,
    pub operation: Option<String>,
}

pub async fn copy_move_task(
    auth: AuthUser,
    State(state): State<Arc<AppState>>,
    Json(body): Json<CopyMoveTaskRequest>,
) -> Result<Json<serde_json::Value>, AppError> {
    crate::domain::permission::check_repo_write_permission(
        state.repos.member.as_ref(),
        &body.src_repo_id,
        auth.user_id,
    )
    .await?;

    auth.ensure_repo_allowed(&body.src_repo_id, true)?;

    let src_dir = safe_normalize_path(&body.src_parent_dir)
        .map_err(|e| AppError::BadRequest(format!("Invalid source path: {e}")))?;
    let dst_dir = safe_normalize_path(&body.dst_parent_dir)
        .map_err(|e| AppError::BadRequest(format!("Invalid destination path: {e}")))?;

    let operation = body.operation.as_deref().unwrap_or("copy");
    let key = match operation {
        "copy" => JobKey::Copy,
        "move" => JobKey::Move,
        other => {
            return Err(AppError::BadRequest(format!("unknown operation: {other}")));
        }
    };

    let first = body
        .src_dirents
        .first()
        .cloned()
        .unwrap_or_else(|| "?".into());
    let description = format!(
        "{} \"{first}\"",
        if key == JobKey::Copy { "Copy" } else { "Move" }
    );
    let total = body.src_dirents.len().max(1) as u64;

    let task_id = state
        .tasks
        .submit(
            key,
            Some(auth.user_id),
            serde_json::json!({
                "repo_id": body.src_repo_id,
                "src_dir": src_dir,
                "dst_dir": dst_dir,
                "file_names": body.src_dirents,
                "email": auth.email,
            }),
            description,
            Some(total),
        )
        .await?;

    Ok(Json(serde_json::json!({"task_id": task_id.as_str()})))
}

#[derive(Deserialize)]
pub struct QueryProgressQuery {
    pub task_id: Option<String>,
}

/// `GET /api/v2.1/query-copy-move-progress/`
pub async fn query_copy_move_progress(
    auth: AuthUser,
    State(state): State<Arc<AppState>>,
    Query(query): Query<QueryProgressQuery>,
) -> Result<Json<serde_json::Value>, AppError> {
    let task_id = query.task_id.unwrap_or_default();
    // Only the owner may read a task: its description carries file names and
    // its failure text can carry paths. A task that is missing, expired or
    // owned by somebody else is reported identically, so the response cannot
    // be used to probe which ids exist.
    let run = state
        .tasks
        .store()
        .get_for(
            &crate::tasks::run::RunId::from_client(task_id),
            Viewer::user(auth.user_id),
        )
        .ok_or_else(|| AppError::NotFound("task not found or expired".into()))?;

    Ok(Json(copy_move_progress(&run)))
}

/// "Copy \"a.txt\"" or "Copy \"a.txt\" and 2 more items".
fn describe(verb: &str, names: &[String]) -> String {
    match names {
        [only] => format!("{verb} \"{only}\""),
        [first, rest @ ..] => format!("{verb} \"{first}\" and {} more items", rest.len()),
        [] => format!("{verb} 0 items"),
    }
}
