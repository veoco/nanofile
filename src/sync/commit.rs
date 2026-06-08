use axum::{
    Json, Router,
    extract::{Path, State},
    http::StatusCode,
};
use sea_orm::{ActiveModelTrait, ColumnTrait, EntityTrait, QueryFilter};
use serde::Serialize;
use std::sync::Arc;

use crate::AppState;
use crate::auth::middleware::SyncAuth;
use crate::entity::{commit, repo};
use crate::error::AppError;

#[derive(Serialize)]
pub struct HeadCommitResponse {
    pub is_corrupted: i32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub head_commit_id: Option<String>,
}

pub fn commit_routes() -> Router<Arc<AppState>> {
    Router::new()
        // Both with and without trailing slash — seaf-daemon uses HEAD/ for
        // update_branch (PUT .../commit/HEAD/?head=...) but HEAD for
        // get_head_commit (GET .../commit/HEAD).
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
    let repo_model = repo::Entity::find_by_id(&repo_id)
        .one(state.db.as_ref())
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
    // The zero commit ID represents an empty repository — return a minimal
    // placeholder commit so the client can proceed with the initial sync.
    if commit_id == "0000000000000000000000000000000000000000" {
        let empty_commit = crate::serialization::commit_json::CommitData {
            commit_id: commit_id.clone(),
            repo_id: repo_id.clone(),
            root_id: "0000000000000000000000000000000000000000".to_string(),
            creator_name: "".to_string(),
            creator: "0000000000000000000000000000000000000000".to_string(),
            description: "".to_string(),
            ctime: 0,
            parent_id: None,
            second_parent_id: None,
            repo_name: None,
            repo_desc: None,
            repo_category: None,
            encrypted: None,
            enc_version: None,
            magic: None,
            key: None,
            version: 1,
        };
        let json = empty_commit.to_compact_json();
        return Ok(json.into_bytes());
    }

    let commit_model = commit::Entity::find()
        .filter(commit::Column::RepoId.eq(&repo_id))
        .filter(commit::Column::CommitId.eq(&commit_id))
        .one(state.db.as_ref())
        .await?
        .ok_or_else(|| AppError::NotFound("commit not found".into()))?;

    // Fetch repo metadata — the seaf-daemon uses repo_name/repo_desc from
    // the commit JSON (via seaf_repo_from_commit) to populate the local
    // library name. Without these fields the library shows as "(unnamed)".
    let repo_model = repo::Entity::find_by_id(&repo_id)
        .one(state.db.as_ref())
        .await?
        .ok_or_else(|| AppError::NotFound("repo not found".into()))?;

    let commit_data = crate::serialization::commit_json::CommitData {
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

    let json = commit_data.to_compact_json();
    Ok(json.into_bytes())
}

pub async fn put_commit(
    State(state): State<Arc<AppState>>,
    _auth: SyncAuth,
    Path((repo_id, commit_id)): Path<(String, String)>,
    body: axum::body::Body,
) -> Result<StatusCode, AppError> {
    let data = axum::body::to_bytes(body, usize::MAX)
        .await
        .map_err(|e| AppError::Internal(e.to_string()))?;

    let commit_data: crate::serialization::commit_json::CommitData = serde_json::from_slice(&data)
        .map_err(|e| AppError::Internal(format!("invalid commit JSON: {}", e)))?;

    if commit_data.repo_id != repo_id {
        return Err(AppError::BadRequest("repo_id mismatch".into()));
    }

    let existing = commit::Entity::find()
        .filter(commit::Column::RepoId.eq(&repo_id))
        .filter(commit::Column::CommitId.eq(&commit_id))
        .one(state.db.as_ref())
        .await?;

    if existing.is_none() {
        let commit_model = commit::ActiveModel {
            id: sea_orm::NotSet,
            repo_id: sea_orm::Set(commit_data.repo_id),
            commit_id: sea_orm::Set(commit_data.commit_id),
            root_id: sea_orm::Set(commit_data.root_id),
            parent_id: sea_orm::Set(commit_data.parent_id),
            second_parent_id: sea_orm::Set(commit_data.second_parent_id),
            creator_name: sea_orm::Set(commit_data.creator_name),
            description: sea_orm::Set(commit_data.description),
            ctime: sea_orm::Set(commit_data.ctime),
            version: sea_orm::Set(commit_data.version as i8),
        };
        commit::Entity::insert(commit_model)
            .exec(state.db.as_ref())
            .await?;
    }

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

    // Read the new commit (checks existence + gets parent_id for conflict detection).
    let new_commit = commit::Entity::find()
        .filter(commit::Column::RepoId.eq(&repo_id))
        .filter(commit::Column::CommitId.eq(new_head))
        .one(state.db.as_ref())
        .await?
        .ok_or_else(|| AppError::BadRequest("commit not found".into()))?;

    let current_head = repo::Entity::find_by_id(&repo_id)
        .one(state.db.as_ref())
        .await?
        .ok_or_else(|| AppError::NotFound("repo not found".into()))?
        .head_commit_id;

    // Conflict detection: the new commit's parent MUST match the current
    // HEAD (unless this is setting the same commit — which is idempotent).
    // This prevents a sync client from overwriting HEAD with a stale commit.
    // Allow setting HEAD to the same commit (idempotent).
    //
    // When there is no existing HEAD (current_head is None, meaning the repo
    // was created via REST API without an initial commit), there's nothing
    // to conflict with — accept any first commit. Without this, the first
    // sync from a client always fails because get_head_commit() returns the
    // "0000..." sentinel for empty repos, but the DB stores NULL, causing
    // a false mismatch between the client's parent_id and current_head.
    let is_same_commit = current_head.as_deref() == Some(new_head.as_str());
    if !is_same_commit && current_head.is_some() && new_commit.parent_id != current_head {
        return Err(AppError::Conflict(
            "commit parent_id does not match current HEAD".into(),
        ));
    }

    let mut repo_active: repo::ActiveModel = repo::Entity::find_by_id(&repo_id)
        .one(state.db.as_ref())
        .await?
        .ok_or_else(|| AppError::NotFound("repo not found".into()))?
        .into();

    repo_active.head_commit_id = sea_orm::Set(Some(new_head.clone()));
    repo_active.update(state.db.as_ref()).await?;

    // Invalidate the path cache so subsequent reads reflect the new HEAD.
    state.path_cache.clear_repo(&repo_id);

    // Fire repo-update notification to subscribed WebSocket clients.
    if let Some(ref notif_mgr) = state.notification_manager {
        let event =
            crate::notification::events::RepoUpdateEvent::new(repo_id.clone(), new_head.clone());
        notif_mgr.notify(event).await;
    }

    Ok(StatusCode::OK)
}
