use axum::{Json, extract::Query, extract::State};
use serde::Deserialize;
use std::sync::Arc;

use crate::AppState;
use crate::activity_log;
use crate::auth::middleware::AuthUser;
use crate::error::AppError;
use crate::serialization::S_IFDIR;
use crate::serialization::fs_json::DirEntryData;
use crate::storage::file_ops::FileOps;

use super::task_manager::TaskState;

/// POST /api/v2.1/repos/async-batch-copy-item/
///
/// Async batch copy — creates a task and runs the copy in the background.
pub async fn async_batch_copy_item(
    auth: AuthUser,
    State(state): State<Arc<AppState>>,
    Json(body): Json<super::batch::SyncBatchCopyRequest>,
) -> Result<Json<serde_json::Value>, AppError> {
    // Permission check on source repo
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
    let body_clone = super::batch::SyncBatchCopyRequest {
        src_repo_id: body.src_repo_id,
        src_parent_dir: body.src_parent_dir,
        src_dirents: body.src_dirents,
        dst_repo_id: body.dst_repo_id,
        dst_parent_dir: body.dst_parent_dir,
    };
    let modifier = auth.email.clone();
    let user_id = auth.user_id;
    let tid = task_id.clone();

    tokio::spawn(async move {
        run_copy_task(&state_clone, &tid, &modifier, user_id, &body_clone).await;
    });

    Ok(Json(serde_json::json!({"task_id": task_id})))
}

async fn run_copy_task(
    state: &Arc<AppState>,
    task_id: &str,
    modifier: &str,
    user_id: i32,
    body: &super::batch::SyncBatchCopyRequest,
) {
    state.task_manager.start_processing(task_id);

    // Cross-repo not supported
    if body.src_repo_id != body.dst_repo_id {
        state
            .task_manager
            .fail_task(task_id, "cross-repo copy not supported".into());
        return;
    }

    let db = state.db.as_ref();

    let head_root_id = match super::batch::get_head_root_id(db, &body.src_repo_id).await {
        Ok(id) => id,
        Err(e) => {
            state.task_manager.fail_task(task_id, e.to_string());
            return;
        }
    };

    let src_dir = super::batch::normalize_path(&body.src_parent_dir);
    let dst_dir = super::batch::normalize_path(&body.dst_parent_dir);

    // Resolve source parent
    let src_parent_fs_id =
        match crate::storage::resolve_fs_id(db, &body.src_repo_id, &head_root_id, &src_dir).await {
            Ok(id) => id,
            Err(e) => {
                state.task_manager.fail_task(task_id, e.to_string());
                return;
            }
        };

    let src_parent_data =
        match crate::storage::read_fs_dir_data(db, &body.src_repo_id, &src_parent_fs_id).await {
            Ok(d) => d,
            Err(e) => {
                state.task_manager.fail_task(task_id, e.to_string());
                return;
            }
        };

    // Collect source entries
    let mut new_entries: Vec<DirEntryData> = Vec::new();
    let now = chrono::Utc::now().timestamp();

    for name in &body.src_dirents {
        match src_parent_data.dirents.iter().find(|d| d.name == *name) {
            Some(entry) => new_entries.push(DirEntryData {
                id: entry.id.clone(),
                mode: entry.mode,
                modifier: modifier.to_string(),
                mtime: now,
                name: entry.name.clone(),
                size: entry.size,
            }),
            None => {
                state
                    .task_manager
                    .fail_task(task_id, format!("source file not found: {name}"));
                return;
            }
        }
    }

    // Resolve destination parent
    let dst_parent_fs_id =
        match crate::storage::resolve_fs_id(db, &body.src_repo_id, &head_root_id, &dst_dir).await {
            Ok(id) => id,
            Err(e) => {
                state.task_manager.fail_task(task_id, e.to_string());
                return;
            }
        };

    // Add entries to destination
    if FileOps::update_dir_tree_and_commit(
        db,
        &body.src_repo_id,
        &dst_dir,
        &dst_parent_fs_id,
        modifier,
        "Async copy",
        crate::storage::file_ops::EMPTY_ANCESTOR_CHAIN,
        |dirents| {
            for entry in &new_entries {
                if dirents.iter().any(|d| d.name == entry.name) {
                    let unique_name = super::batch::generate_unique_filename(dirents, &entry.name);
                    dirents.push(DirEntryData {
                        name: unique_name,
                        ..entry.clone()
                    });
                } else {
                    dirents.push(entry.clone());
                }
            }
            Ok(())
        },
    )
    .await
    .is_err()
    {
        state
            .task_manager
            .fail_task(task_id, "async copy failed".into());
        return;
    }

    // Log activity for each copied item (best-effort).
    for entry in &new_entries {
        let fp = if dst_dir == "/" {
            format!("/{}", entry.name)
        } else {
            format!("{}/{}", dst_dir, entry.name)
        };
        let obj_type = if entry.mode & S_IFDIR != 0 {
            "dir"
        } else {
            "file"
        };
        activity_log::log_activity(
            db,
            &body.src_repo_id,
            "create",
            obj_type,
            &fp,
            user_id,
            None,
        )
        .await;
    }
    state.task_manager.complete_task(task_id);
}

/// POST /api/v2.1/repos/async-batch-move-item/
///
/// Async batch move — creates a task and runs the move in the background.
pub async fn async_batch_move_item(
    auth: AuthUser,
    State(state): State<Arc<AppState>>,
    Json(body): Json<super::batch::BatchMoveRequest>,
) -> Result<Json<serde_json::Value>, AppError> {
    // Permission check on source repo
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
    let body_clone = super::batch::BatchMoveRequest {
        src_repo_id: body.src_repo_id,
        src_parent_dir: body.src_parent_dir,
        src_dirents: body.src_dirents,
        dst_repo_id: body.dst_repo_id,
        dst_parent_dir: body.dst_parent_dir,
    };
    let modifier = auth.email.clone();
    let user_id = auth.user_id;
    let tid = task_id.clone();

    tokio::spawn(async move {
        run_move_task(&state_clone, &tid, &modifier, user_id, &body_clone).await;
    });

    Ok(Json(serde_json::json!({"task_id": task_id})))
}

async fn run_move_task(
    state: &Arc<AppState>,
    task_id: &str,
    modifier: &str,
    user_id: i32,
    body: &super::batch::BatchMoveRequest,
) {
    state.task_manager.start_processing(task_id);

    if body.src_repo_id != body.dst_repo_id {
        state
            .task_manager
            .fail_task(task_id, "cross-repo move not supported".into());
        return;
    }

    let db = state.db.as_ref();
    let repo_id = &body.src_repo_id;

    let head_root_id = match super::batch::get_head_root_id(db, repo_id).await {
        Ok(id) => id,
        Err(e) => {
            state.task_manager.fail_task(task_id, e.to_string());
            return;
        }
    };

    let src_dir = super::batch::normalize_path(&body.src_parent_dir);
    let dst_dir = super::batch::normalize_path(&body.dst_parent_dir);

    // Resolve source parent
    let src_parent_fs_id =
        match crate::storage::resolve_fs_id(db, repo_id, &head_root_id, &src_dir).await {
            Ok(id) => id,
            Err(e) => {
                state.task_manager.fail_task(task_id, e.to_string());
                return;
            }
        };

    let src_parent_data =
        match crate::storage::read_fs_dir_data(db, repo_id, &src_parent_fs_id).await {
            Ok(d) => d,
            Err(e) => {
                state.task_manager.fail_task(task_id, e.to_string());
                return;
            }
        };

    // Collect entries to move
    let mut entries_to_move: Vec<DirEntryData> = Vec::new();
    let now = chrono::Utc::now().timestamp();

    for name in &body.src_dirents {
        match src_parent_data.dirents.iter().find(|d| d.name == *name) {
            Some(entry) => entries_to_move.push(DirEntryData {
                id: entry.id.clone(),
                mode: entry.mode,
                modifier: modifier.to_string(),
                mtime: now,
                name: entry.name.clone(),
                size: entry.size,
            }),
            None => {
                state
                    .task_manager
                    .fail_task(task_id, format!("source not found: {name}"));
                return;
            }
        }
    }

    // Resolve destination
    let _dst_parent_fs_id =
        match crate::storage::resolve_fs_id(db, repo_id, &head_root_id, &dst_dir).await {
            Ok(id) => id,
            Err(e) => {
                state.task_manager.fail_task(task_id, e.to_string());
                return;
            }
        };

    // Step 1: Remove from source
    let src_names: Vec<String> = entries_to_move.iter().map(|e| e.name.clone()).collect();
    let intermediate_root = match FileOps::update_dir_tree_no_commit(
        db,
        repo_id,
        &src_dir,
        &src_parent_fs_id,
        crate::storage::file_ops::EMPTY_ANCESTOR_CHAIN,
        |dirents| {
            dirents.retain(|d| !src_names.contains(&d.name));
            Ok(())
        },
    )
    .await
    {
        Ok(r) => r,
        Err(e) => {
            state.task_manager.fail_task(task_id, e.to_string());
            return;
        }
    };

    if let Err(e) =
        FileOps::create_commit(db, repo_id, &intermediate_root, modifier, "Move (remove)").await
    {
        state.task_manager.fail_task(task_id, e.to_string());
        return;
    }

    // Step 2: Re-read head, add to destination
    let new_head_root = match super::batch::get_head_root_id(db, repo_id).await {
        Ok(id) => id,
        Err(e) => {
            state.task_manager.fail_task(task_id, e.to_string());
            return;
        }
    };

    let new_dst_fs_id =
        match crate::storage::resolve_fs_id(db, repo_id, &new_head_root, &dst_dir).await {
            Ok(id) => id,
            Err(e) => {
                state.task_manager.fail_task(task_id, e.to_string());
                return;
            }
        };

    if FileOps::update_dir_tree_and_commit(
        db,
        repo_id,
        &dst_dir,
        &new_dst_fs_id,
        modifier,
        "Move (add)",
        crate::storage::file_ops::EMPTY_ANCESTOR_CHAIN,
        |dirents| {
            for entry in &entries_to_move {
                if dirents.iter().any(|d| d.name == entry.name) {
                    let unique_name = super::batch::generate_unique_filename(dirents, &entry.name);
                    dirents.push(DirEntryData {
                        name: unique_name,
                        ..entry.clone()
                    });
                } else {
                    dirents.push(entry.clone());
                }
            }
            Ok(())
        },
    )
    .await
    .is_err()
    {
        state
            .task_manager
            .fail_task(task_id, "async move (add) failed".into());
        return;
    }

    // Log activity for each moved item (best-effort).
    for entry in &entries_to_move {
        let new_fp = if dst_dir == "/" {
            format!("/{}", entry.name)
        } else {
            format!("{}/{}", dst_dir, entry.name)
        };
        let old_fp = if src_dir == "/" {
            format!("/{}", entry.name)
        } else {
            format!("{}/{}", src_dir, entry.name)
        };
        let obj_type = if entry.mode & S_IFDIR != 0 {
            "dir"
        } else {
            "file"
        };
        activity_log::log_activity(
            db,
            repo_id,
            "move",
            obj_type,
            &new_fp,
            user_id,
            Some(&old_fp),
        )
        .await;
    }
    state.task_manager.complete_task(task_id);
}

/// POST /api/v2.1/copy-move-task/
///
/// Single-item async copy/move. Creates a task and runs in the background.
pub async fn copy_move_task(
    auth: AuthUser,
    State(state): State<Arc<AppState>>,
    Json(body): Json<CopyMoveTaskRequest>,
) -> Result<Json<serde_json::Value>, AppError> {
    // Permission check on source repo
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
    let modifier = auth.email.clone();
    let user_id = auth.user_id;
    let tid = task_id.clone();

    match operation.as_str() {
        "copy" => {
            let req = super::batch::SyncBatchCopyRequest {
                src_repo_id: body.src_repo_id.clone(),
                src_parent_dir: body.src_parent_dir.clone(),
                src_dirents: body.src_dirents.clone(),
                dst_repo_id: body.dst_repo_id.clone(),
                dst_parent_dir: body.dst_parent_dir.clone(),
            };
            tokio::spawn(async move {
                run_copy_task(&state_clone, &tid, &modifier, user_id, &req).await;
            });
        }
        "move" => {
            let req = super::batch::BatchMoveRequest {
                src_repo_id: body.src_repo_id.clone(),
                src_parent_dir: body.src_parent_dir.clone(),
                src_dirents: body.src_dirents.clone(),
                dst_repo_id: body.dst_repo_id.clone(),
                dst_parent_dir: body.dst_parent_dir.clone(),
            };
            tokio::spawn(async move {
                run_move_task(&state_clone, &tid, &modifier, user_id, &req).await;
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
pub struct CopyMoveTaskRequest {
    pub src_repo_id: String,
    pub src_parent_dir: String,
    pub src_dirents: Vec<String>,
    pub dst_repo_id: String,
    pub dst_parent_dir: String,
    pub operation: Option<String>,
}

/// GET /api/v2.1/query-copy-move-progress/
///
/// Poll the progress of an async copy/move task.
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
                "state": state_str,
                "done": done,
                "failed": failed,
                "description": error_msg,
                "total": t.total,
                "done_count": t.done_count,
                "failed_count": if failed { 1 } else { 0 },
                "cancelable": false,
            })))
        }
        None => Ok(Json(serde_json::json!({
            "state": "completed",
            "done": true,
            "failed": false,
            "description": "",
            "total": 0,
            "done_count": 0,
            "failed_count": 0,
            "cancelable": false,
        }))),
    }
}

#[derive(Deserialize)]
pub struct QueryProgressQuery {
    pub task_id: Option<String>,
}
