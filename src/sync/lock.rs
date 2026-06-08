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
use crate::entity::{locked_file, sync_token, user};
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

    locked_file::Entity::delete_many()
        .filter(locked_file::Column::RepoId.eq(&repo_id))
        .filter(locked_file::Column::Path.eq(path))
        .exec(state.db.as_ref())
        .await?;

    Ok(Json(LockResponse { success: true }))
}

#[derive(Serialize)]
pub struct LockedFileEntry {
    pub repo_id: String,
    pub path: String,
    pub lock_owner: String,
    pub lock_owner_email: String,
    pub locked_at: i64,
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
        let token_valid = sync_token::Entity::find()
            .filter(sync_token::Column::Token.eq(&req.token))
            .filter(sync_token::Column::RepoId.eq(&req.repo_id))
            .one(state.db.as_ref())
            .await?
            .is_some();

        let files = if token_valid {
            let locked = locked_file::Entity::find()
                .filter(locked_file::Column::RepoId.eq(&req.repo_id))
                .all(state.db.as_ref())
                .await?;

            let mut entries = Vec::new();
            for entry in locked {
                let user_record = user::Entity::find_by_id(entry.user_id)
                    .one(state.db.as_ref())
                    .await?;
                entries.push(LockedFileEntry {
                    repo_id: entry.repo_id,
                    path: entry.path,
                    lock_owner: user_record
                        .as_ref()
                        .map(|u| u.email.clone())
                        .unwrap_or_default(),
                    lock_owner_email: user_record.map(|u| u.email).unwrap_or_default(),
                    locked_at: entry.locked_at,
                });
            }
            entries
        } else {
            vec![]
        };

        results.push(LockedFilesRes {
            repo_id: req.repo_id.clone(),
            ts: req.ts,
            locked_files: files,
        });
    }

    Ok((StatusCode::OK, Json(results)))
}

pub fn lock_routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/{repo_id}/lock-file", axum::routing::post(lock_file))
        .route("/{repo_id}/unlock-file", axum::routing::post(unlock_file))
        .route("/locked-files", axum::routing::post(list_locked_files_post))
}
