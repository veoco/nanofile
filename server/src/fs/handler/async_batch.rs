use axum::{
    Json,
    extract::{Query, State},
};
use serde::Deserialize;
use std::sync::Arc;

use crate::AppState;
use crate::auth::middleware::AuthUser;
use crate::common::util::normalize_path;
use crate::error::AppError;
use crate::fs::service::fileops::FileOpsService;
use crate::fs::task_manager::TaskState;

pub async fn async_batch_copy_item(
    auth: AuthUser,
    State(state): State<Arc<AppState>>,
    Json(body): Json<super::batch::SyncBatchCopyRequest>,
) -> Result<Json<serde_json::Value>, AppError> {
    crate::storage::check_repo_write_permission(state.db.as_ref(), &body.src_repo_id, auth.user_id)
        .await?;

    let task_id = uuid::Uuid::new_v4().to_string();
    let description = if body.src_dirents.len() == 1 {
        format!("Copy \"{}\"", body.src_dirents[0])
    } else {
        format!(
            "Copy \"{}\" and {} more items",
            body.src_dirents[0],
            body.src_dirents.len() - 1
        )
    };

    state.task_manager.create_task(
        task_id.clone(),
        "copy",
        body.src_dirents.len(),
        &description,
    );

    let state_clone = state.clone();
    let repo_id = body.src_repo_id;
    let src_dir = normalize_path(&body.src_parent_dir);
    let dst_dir = normalize_path(&body.dst_parent_dir);
    let file_names = body.src_dirents;
    let email = auth.email.clone();
    let uid = auth.user_id;
    let tid = task_id.clone();

    tokio::spawn(async move {
        run_copy_task(
            &state_clone,
            &tid,
            &repo_id,
            &src_dir,
            &dst_dir,
            &file_names,
            &email,
            uid,
        )
        .await;
    });

    Ok(Json(serde_json::json!({"task_id": task_id})))
}

async fn run_copy_task(
    state: &Arc<AppState>,
    task_id: &str,
    repo_id: &str,
    src_dir: &str,
    dst_dir: &str,
    file_names: &[String],
    email: &str,
    user_id: i32,
) {
    state.task_manager.start_processing(task_id);

    let svc = FileOpsService::new(
        state.db.clone(),
        state.block_store.clone(),
        state.indexer.clone(),
    );

    match svc
        .batch_copy(repo_id, src_dir, dst_dir, file_names, email, user_id)
        .await
    {
        Ok(_) => state.task_manager.complete_task(task_id),
        Err(e) => state.task_manager.fail_task(task_id, e.to_string()),
    }
}

pub async fn async_batch_move_item(
    auth: AuthUser,
    State(state): State<Arc<AppState>>,
    Json(body): Json<super::batch::BatchMoveRequest>,
) -> Result<Json<serde_json::Value>, AppError> {
    crate::storage::check_repo_write_permission(state.db.as_ref(), &body.src_repo_id, auth.user_id)
        .await?;

    let task_id = uuid::Uuid::new_v4().to_string();
    let description = if body.src_dirents.len() == 1 {
        format!("Move \"{}\"", body.src_dirents[0])
    } else {
        format!(
            "Move \"{}\" and {} more items",
            body.src_dirents[0],
            body.src_dirents.len() - 1
        )
    };

    state.task_manager.create_task(
        task_id.clone(),
        "move",
        body.src_dirents.len(),
        &description,
    );

    let state_clone = state.clone();
    let repo_id = body.src_repo_id;
    let src_dir = normalize_path(&body.src_parent_dir);
    let dst_dir = normalize_path(&body.dst_parent_dir);
    let file_names = body.src_dirents;
    let email = auth.email.clone();
    let uid = auth.user_id;
    let tid = task_id.clone();

    tokio::spawn(async move {
        run_move_task(
            &state_clone,
            &tid,
            &repo_id,
            &src_dir,
            &dst_dir,
            &file_names,
            &email,
            uid,
        )
        .await;
    });

    Ok(Json(serde_json::json!({"task_id": task_id})))
}

async fn run_move_task(
    state: &Arc<AppState>,
    task_id: &str,
    repo_id: &str,
    src_dir: &str,
    dst_dir: &str,
    file_names: &[String],
    email: &str,
    user_id: i32,
) {
    state.task_manager.start_processing(task_id);

    let svc = FileOpsService::new(
        state.db.clone(),
        state.block_store.clone(),
        state.indexer.clone(),
    );

    match svc
        .batch_move(repo_id, src_dir, dst_dir, file_names, email, user_id)
        .await
    {
        Ok(_) => state.task_manager.complete_task(task_id),
        Err(e) => state.task_manager.fail_task(task_id, e.to_string()),
    }
}

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
    crate::storage::check_repo_write_permission(state.db.as_ref(), &body.src_repo_id, auth.user_id)
        .await?;

    let operation = body.operation.as_deref().unwrap_or("copy").to_string();
    let task_id = uuid::Uuid::new_v4().to_string();
    let description = format!(
        "{} \"{}\"",
        if operation == "copy" { "Copy" } else { "Move" },
        body.src_dirents.first().map(|s| s.as_str()).unwrap_or("?")
    );

    state
        .task_manager
        .create_task(task_id.clone(), &operation, 1, &description);

    let state_clone = state.clone();
    let repo_id = body.src_repo_id;
    let src_dir = normalize_path(&body.src_parent_dir);
    let dst_dir = normalize_path(&body.dst_parent_dir);
    let file_names = body.src_dirents;
    let email = auth.email.clone();
    let uid = auth.user_id;
    let tid = task_id.clone();

    match operation.as_str() {
        "copy" => {
            tokio::spawn(async move {
                run_copy_task(
                    &state_clone,
                    &tid,
                    &repo_id,
                    &src_dir,
                    &dst_dir,
                    &file_names,
                    &email,
                    uid,
                )
                .await;
            });
        }
        "move" => {
            tokio::spawn(async move {
                run_move_task(
                    &state_clone,
                    &tid,
                    &repo_id,
                    &src_dir,
                    &dst_dir,
                    &file_names,
                    &email,
                    uid,
                )
                .await;
            });
        }
        _ => {
            state
                .task_manager
                .fail_task(&task_id, format!("unknown operation: {operation}"));
            return Err(AppError::BadRequest(format!(
                "unknown operation: {operation}"
            )));
        }
    }

    Ok(Json(serde_json::json!({"task_id": task_id})))
}

#[derive(Deserialize)]
pub struct QueryProgressQuery {
    pub task_id: Option<String>,
}

pub async fn query_copy_move_progress(
    _auth: AuthUser,
    State(state): State<Arc<AppState>>,
    Query(query): Query<QueryProgressQuery>,
) -> Result<Json<serde_json::Value>, AppError> {
    let task_id = query.task_id.unwrap_or_default();
    let task = state.task_manager.get_task(&task_id);

    match task {
        Some(t) => {
            let (state_str, done, failed) = match &t.state {
                TaskState::Pending => ("pending", false, false),
                TaskState::Processing => ("processing", false, false),
                TaskState::Completed => ("completed", true, false),
                TaskState::Failed(_) => ("failed", true, true),
            };
            let error_msg = match &t.state {
                TaskState::Failed(msg) => msg.clone(),
                _ => String::new(),
            };

            Ok(Json(serde_json::json!({
                "state": state_str, "done": done, "failed": failed,
                "description": error_msg, "total": t.total,
                "done_count": t.done_count, "failed_count": if failed { 1 } else { 0 },
                "cancelable": false,
            })))
        }
        None => Ok(Json(serde_json::json!({
            "state": "completed", "done": true, "failed": false,
            "description": "", "total": 0, "done_count": 0, "failed_count": 0, "cancelable": false,
        }))),
    }
}
