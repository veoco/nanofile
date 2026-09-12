use axum::{
    Json, Router,
    extract::{Path, State},
    http::StatusCode,
};
use serde::Serialize;
use std::sync::Arc;

use crate::AppState;
use crate::middleware::auth::SyncAuth;
use base::common::EMPTY_SHA1;
use base::error::AppError;

/// A commit is a small JSON document (commit id, root/parent ids, message);
/// a few KB in practice. Cap the request to avoid unbounded buffering.
const MAX_COMMIT_BYTES: usize = 4 * 1024 * 1024;

#[derive(Serialize)]
pub struct HeadCommitResponse {
    pub is_corrupted: i32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub head_commit_id: Option<String>,
}

pub fn commit_routes() -> Router<Arc<AppState>> {
    Router::new()
        .route(
            "/{repo_id}/commit/HEAD",
            axum::routing::get(get_head_commit).put(update_branch),
        )
        .route(
            "/{repo_id}/commit/HEAD/",
            axum::routing::get(get_head_commit).put(update_branch),
        )
        .route(
            "/{repo_id}/commit/{commit_id}",
            axum::routing::get(get_commit).put(put_commit),
        )
}

pub async fn get_head_commit(
    State(state): State<Arc<AppState>>,
    auth: SyncAuth,
    Path(repo_id): Path<String>,
) -> Result<Json<HeadCommitResponse>, AppError> {
    // Defense in depth: `SyncAuth` also binds the token to the URL repo (see
    // `fs_id_list` in `sync/fs.rs`).
    crate::domain::permission::check_repo_read_permission(
        state.repos.member.as_ref(),
        &repo_id,
        auth.user_id,
    )
    .await?;

    let svc = state.sync_service();
    let repo_model = svc
        .find_repo(&repo_id)
        .await?
        .ok_or_else(|| AppError::NotFound("repo not found".into()))?;

    let head_commit_id = repo_model
        .head_commit_id
        .unwrap_or_else(|| EMPTY_SHA1.to_string());

    Ok(Json(HeadCommitResponse {
        is_corrupted: 0,
        head_commit_id: Some(head_commit_id),
    }))
}

pub async fn get_commit(
    State(state): State<Arc<AppState>>,
    auth: SyncAuth,
    Path((repo_id, commit_id)): Path<(String, String)>,
) -> Result<Vec<u8>, AppError> {
    // Membership check (see `get_head_commit`). The commit body carries repo
    // metadata (name, description, creator), so it must not be readable
    // cross-repo.
    crate::domain::permission::check_repo_read_permission(
        state.repos.member.as_ref(),
        &repo_id,
        auth.user_id,
    )
    .await?;

    let svc = state.sync_service();
    let repo_model = svc
        .find_repo(&repo_id)
        .await?
        .ok_or_else(|| AppError::NotFound("repo not found".into()))?;

    if commit_id == EMPTY_SHA1 {
        let empty_commit = base::common::CommitData {
            commit_id: commit_id.clone(),
            repo_id: repo_id.clone(),
            root_id: EMPTY_SHA1.to_string(),
            creator_name: "".to_string(),
            creator: EMPTY_SHA1.to_string(),
            description: "".to_string(),
            ctime: 0,
            parent_id: None,
            second_parent_id: None,
            repo_name: Some(repo_model.name.clone()),
            repo_desc: Some(repo_model.description.clone()),
            repo_category: None,
            encrypted: if repo_model.encrypted == 1 {
                Some("true".to_string())
            } else {
                None
            },
            enc_version: Some(repo_model.enc_version as i32),
            magic: repo_model.magic.clone(),
            key: repo_model.random_key.clone(),
            version: 1,
        };
        let json = crate::domain::commit::to_json(&empty_commit);
        return Ok(json.into_bytes());
    }

    let commit_model = svc
        .find_commit(&repo_id, &commit_id)
        .await?
        .ok_or_else(|| AppError::NotFound("commit not found".into()))?;

    let commit_data = base::common::CommitData {
        commit_id: commit_model.commit_id.clone(),
        repo_id: commit_model.repo_id.clone(),
        root_id: commit_model.root_id.clone(),
        creator_name: commit_model.creator_name.clone(),
        creator: commit_model.creator.clone(),
        description: commit_model.description.clone(),
        ctime: commit_model.ctime,
        parent_id: commit_model.parent_id.clone(),
        second_parent_id: commit_model.second_parent_id.clone(),
        repo_name: Some(repo_model.name.clone()),
        repo_desc: Some(repo_model.description.clone()),
        repo_category: None,
        encrypted: if repo_model.encrypted == 1 {
            Some("true".to_string())
        } else {
            None
        },
        enc_version: Some(repo_model.enc_version as i32),
        magic: repo_model.magic.clone(),
        key: repo_model.random_key.clone(),
        version: commit_model.version as i32,
    };

    let json = crate::domain::commit::to_json(&commit_data);
    Ok(json.into_bytes())
}

pub async fn put_commit(
    State(state): State<Arc<AppState>>,
    auth: SyncAuth,
    Path((repo_id, _commit_id)): Path<(String, String)>,
    body: axum::body::Body,
) -> Result<StatusCode, AppError> {
    // Writing commit objects mutates the repo's object store; a read-only
    // member must not be able to inject data. Checked before the body is read.
    crate::domain::permission::check_repo_write_permission(
        state.repos.member.as_ref(),
        &repo_id,
        auth.user_id,
    )
    .await?;

    let data = axum::body::to_bytes(body, MAX_COMMIT_BYTES)
        .await
        .map_err(|e| AppError::Internal(e.to_string()))?;

    let commit_data: base::common::CommitData = serde_json::from_slice(&data)
        .map_err(|e| AppError::Internal(format!("invalid commit JSON: {}", e)))?;

    if commit_data.repo_id != repo_id {
        return Err(AppError::BadRequest("repo_id mismatch".into()));
    }

    state.sync_service().put_commit(&commit_data).await?;
    Ok(StatusCode::OK)
}

pub async fn update_branch(
    State(state): State<Arc<AppState>>,
    _auth: SyncAuth,
    Path(repo_id): Path<String>,
    axum::extract::Query(params): axum::extract::Query<std::collections::HashMap<String, String>>,
) -> Result<StatusCode, AppError> {
    let new_head = params
        .get("head")
        .ok_or_else(|| AppError::BadRequest("missing head parameter".into()))?;

    if new_head.len() != 40 || !new_head.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err(AppError::BadRequest("invalid commit id".into()));
    }

    crate::domain::permission::check_repo_write_permission(
        state.repos.member.as_ref(),
        &repo_id,
        _auth.user_id,
    )
    .await?;

    let svc = state.sync_service();
    let new_commit = svc
        .find_commit(&repo_id, new_head)
        .await?
        .ok_or_else(|| AppError::Internal("commit not found".into()))?;

    let commit_desc = new_commit.description.clone();
    drop(new_commit);

    let _ = svc
        .update_branch(&repo_id, new_head, _auth.user_id, &commit_desc)
        .await?;

    // The blocks and FS objects this client uploaded are now referenced by the
    // new head, so drop its uncommitted-block reservation for the repo. Blocks
    // it uploaded but never committed are left to GC — the reservation was what
    // bounded the write loop, and holding it after a commit would double-count
    // the bytes that are now part of the repo's size.
    crate::service::fs::quota::release_repo_reservation(&state.repos, _auth.user_id, &repo_id);
    // The commit changed the repo's size, so the cached per-user usage snapshot
    // must not be reused by the next quota check.
    crate::service::fs::quota::invalidate_user(&state.repos, _auth.user_id);

    Ok(StatusCode::OK)
}
