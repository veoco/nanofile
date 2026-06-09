use axum::{
    Json, Router,
    extract::{Path, Query, State},
    http::StatusCode,
};
use sea_orm::{ActiveModelTrait, ColumnTrait, EntityTrait, QueryFilter, Set};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

use crate::AppState;
use crate::auth::middleware::SyncAuth;
use crate::entity::{file_lock_timestamp, locked_file, sync_token};
use crate::error::AppError;

#[derive(Deserialize)]
pub struct LockQuery {
    pub p: Option<String>,
}

#[derive(Serialize)]
pub struct LockResponse {
    pub success: bool,
}

/// `POST /seafhttp/repo/{repo_id}/lock-file?p=path`
pub async fn lock_file(
    _auth: SyncAuth,
    State(state): State<Arc<AppState>>,
    Path(repo_id): Path<String>,
    Query(query): Query<LockQuery>,
) -> Result<Json<LockResponse>, AppError> {
    // Permission check: only users with write access can lock files.
    crate::storage::check_repo_write_permission(state.db.as_ref(), &repo_id, _auth.user_id).await?;

    let path = query
        .p
        .as_deref()
        .ok_or_else(|| AppError::BadRequest("path required".into()))?;
    let now = chrono::Utc::now().timestamp();

    let existing = locked_file::Entity::find()
        .filter(locked_file::Column::RepoId.eq(&repo_id))
        .filter(locked_file::Column::Path.eq(path))
        .one(state.db.as_ref())
        .await?;

    match existing {
        Some(record) => {
            let mut active: locked_file::ActiveModel = record.into();
            active.user_id = Set(_auth.user_id);
            active.locked_at = Set(now);
            active.update(state.db.as_ref()).await?;
        }
        None => {
            locked_file::Entity::insert(locked_file::ActiveModel {
                id: sea_orm::NotSet,
                repo_id: Set(repo_id.clone()),
                path: Set(path.to_string()),
                user_id: Set(_auth.user_id),
                locked_at: Set(now),
                lock_owner_name: Set(String::new()),
            })
            .exec(state.db.as_ref())
            .await?;
        }
    }

    // Update the lock timestamp for client cache invalidation.
    crate::storage::upsert_lock_timestamp(state.db.as_ref(), &repo_id).await?;

    Ok(Json(LockResponse { success: true }))
}

/// `POST /seafhttp/repo/{repo_id}/unlock-file?p=path`
pub async fn unlock_file(
    _auth: SyncAuth,
    State(state): State<Arc<AppState>>,
    Path(repo_id): Path<String>,
    Query(query): Query<LockQuery>,
) -> Result<Json<LockResponse>, AppError> {
    // Permission check: only users with write access can unlock files.
    crate::storage::check_repo_write_permission(state.db.as_ref(), &repo_id, _auth.user_id).await?;

    let path = query
        .p
        .as_deref()
        .ok_or_else(|| AppError::BadRequest("path required".into()))?;

    locked_file::Entity::delete_many()
        .filter(locked_file::Column::RepoId.eq(&repo_id))
        .filter(locked_file::Column::Path.eq(path))
        .exec(state.db.as_ref())
        .await?;

    // Update the lock timestamp for client cache invalidation.
    crate::storage::upsert_lock_timestamp(state.db.as_ref(), &repo_id).await?;

    Ok(Json(LockResponse { success: true }))
}

#[derive(Serialize)]
pub struct LockedFileEntry {
    pub path: String,
    /// 1 = current user locked the file, 0 = locked by another user.
    /// The daemon uses this to decide worktree writability.
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

/// `POST /seafhttp/repo/locked-files`
///
/// Batch locked-files query used by seaf-daemon. Accepts a JSON array of
/// `{repo_id, token, ts}` and returns locked file entries per repo.
/// Daemon sends without Content-Type, so parse raw body manually.
pub async fn list_locked_files_post(
    State(state): State<Arc<AppState>>,
    body: String,
) -> Result<(StatusCode, Json<Vec<LockedFilesRes>>), AppError> {
    let requests: Vec<LockedFilesReq> = serde_json::from_str(&body)
        .map_err(|e| AppError::BadRequest(format!("invalid JSON: {}", e)))?;
    let mut results = Vec::new();
    for req in &requests {
        // Look up the sync token to get the token user_id for `by_me` computation.
        let token_record = sync_token::Entity::find()
            .filter(sync_token::Column::Token.eq(&req.token))
            .filter(sync_token::Column::RepoId.eq(&req.repo_id))
            .one(state.db.as_ref())
            .await?;

        let token_valid = token_record.is_some();
        let token_user_id = token_record.as_ref().map(|t| t.user_id);

        // Get the actual lock timestamp for this repo (used for client cache invalidation).
        let lock_ts = if token_valid {
            file_lock_timestamp::Entity::find()
                .filter(file_lock_timestamp::Column::RepoId.eq(&req.repo_id))
                .one(state.db.as_ref())
                .await?
                .map(|t| t.update_time)
                .unwrap_or(0)
        } else {
            0
        };

        let files = if token_valid {
            let locked = locked_file::Entity::find()
                .filter(locked_file::Column::RepoId.eq(&req.repo_id))
                .all(state.db.as_ref())
                .await?;

            locked
                .into_iter()
                .map(|entry| LockedFileEntry {
                    path: entry.path,
                    by_me: match token_user_id {
                        Some(tuid) if tuid == entry.user_id => 1,
                        _ => 0,
                    },
                })
                .collect()
        } else {
            vec![]
        };

        results.push(LockedFilesRes {
            repo_id: req.repo_id.clone(),
            ts: lock_ts,
            locked_files: files,
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
