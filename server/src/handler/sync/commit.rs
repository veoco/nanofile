use axum::{
    Json, Router,
    extract::{Path, State},
    http::StatusCode,
};
use serde::Serialize;
use std::sync::Arc;

use crate::AppState;
use crate::middleware::auth::SyncAuth;
use base::error::AppError;

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
    _auth: SyncAuth,
    Path(repo_id): Path<String>,
) -> Result<Json<HeadCommitResponse>, AppError> {
    let svc = state.sync_service();
    let repo_model = svc
        .find_repo(&repo_id)
        .await?
        .ok_or_else(|| AppError::NotFound("repo not found".into()))?;

    let head_commit_id = repo_model
        .head_commit_id
        .unwrap_or_else(|| "0000000000000000000000000000000000000000".to_string());

    Ok(Json(HeadCommitResponse {
        is_corrupted: 0,
        head_commit_id: Some(head_commit_id),
    }))
}

pub async fn get_commit(
    State(state): State<Arc<AppState>>,
    _auth: SyncAuth,
    Path((repo_id, commit_id)): Path<(String, String)>,
) -> Result<Vec<u8>, AppError> {
    let svc = state.sync_service();
    let repo_model = svc
        .find_repo(&repo_id)
        .await?
        .ok_or_else(|| AppError::NotFound("repo not found".into()))?;

    if commit_id == "0000000000000000000000000000000000000000" {
        let empty_commit = base::common::CommitData {
            commit_id: commit_id.clone(),
            repo_id: repo_id.clone(),
            root_id: "0000000000000000000000000000000000000000".to_string(),
            creator_name: "".to_string(),
            creator: "0000000000000000000000000000000000000000".to_string(),
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
        creator: "0000000000000000000000000000000000000000".to_string(),
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
    _auth: SyncAuth,
    Path((repo_id, _commit_id)): Path<(String, String)>,
    body: axum::body::Body,
) -> Result<StatusCode, AppError> {
    let max_bytes = (state.config.server.max_upload_size_mb * 1024 * 1024) as usize;
    let data = axum::body::to_bytes(body, max_bytes)
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

    Ok(StatusCode::OK)
}
