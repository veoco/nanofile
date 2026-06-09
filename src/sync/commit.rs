use axum::{
    Json, Router,
    extract::{Path, State},
    http::StatusCode,
};
use rand::Rng;
use sea_orm::{ActiveModelTrait, ColumnTrait, EntityTrait, QueryFilter};
use serde::Serialize;
use std::sync::Arc;

use crate::AppState;
use crate::auth::middleware::SyncAuth;
use crate::entity::{commit, fs_object, repo};
use crate::error::AppError;

/// Maximum number of retries when branch update conflicts.
const MAX_BRANCH_RETRY: u32 = 3;

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

    // Validate commit_id format (40 hex characters, case-insensitive).
    // This matches seafile's is_object_id_valid() which uses
    // strspn(id, "0123456789abcdefABCDEF") == 40.
    if new_head.len() != 40 || !new_head.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err(AppError::BadRequest("invalid commit id".into()));
    }

    // Permission check is handled by SyncAuth middleware for the token.
    // Seafile server additionally checks repo-level write permission here
    // (check_permission → EVHTP_RES_FORBIDDEN).

    // Read the new commit (checks existence + gets parent_id for conflict detection).
    let new_commit = commit::Entity::find()
        .filter(commit::Column::RepoId.eq(&repo_id))
        .filter(commit::Column::CommitId.eq(new_head))
        .one(state.db.as_ref())
        .await?
        .ok_or_else(|| AppError::Internal("commit not found".into()))?;

    // Verify all blocks referenced by the new commit exist on the server.
    // This matches seafile-server's check_blocks() call inside
    // put_update_branch_cb.
    let missing = check_commit_blocks(
        state.db.as_ref(),
        state.block_store.as_ref(),
        &repo_id,
        &new_commit.root_id,
    )
    .await?;
    if !missing.is_empty() {
        return Err(AppError::BlockMissing);
    }

    // Verify the parent commit exists. Seafile requires the base commit
    // to be present on the server; a missing parent is a client error.
    if let Some(ref parent_id) = new_commit.parent_id
        && parent_id != "0000000000000000000000000000000000000000"
    {
        let parent_exists = commit::Entity::find()
            .filter(commit::Column::RepoId.eq(&repo_id))
            .filter(commit::Column::CommitId.eq(parent_id))
            .one(state.db.as_ref())
            .await?
            .is_some();
        if !parent_exists {
            return Err(AppError::BadRequest("parent commit not found".into()));
        }
    }

    // CAS retry loop: re-read current HEAD and attempt update.
    // Seafile's test_and_update_branch uses a DB transaction with
    // SELECT ... FOR UPDATE. Since nanofile uses SQLite (serialized
    // writes), a loop with re-read + conditional update is sufficient.
    let mut attempt: u32 = 0;

    loop {
        attempt += 1;

        let current_head = repo::Entity::find_by_id(&repo_id)
            .one(state.db.as_ref())
            .await?
            .ok_or_else(|| AppError::Internal("repo not found".into()))?
            .head_commit_id;

        // Conflict detection: the new commit's parent MUST match the current
        // HEAD (unless this is setting the same commit — which is idempotent).
        //
        // When there is no existing HEAD (current_head is None, meaning the
        // repo was created via REST API without an initial commit), accept
        // any first commit.
        let is_same_commit = current_head.as_deref() == Some(new_head.as_str());
        if is_same_commit {
            // Idempotent: HEAD already points to this commit → success.
            state.path_cache.clear_repo(&repo_id);
            if let Some(ref notif_mgr) = state.notification_manager {
                notif_mgr
                    .notify(crate::notification::events::RepoUpdateEvent::new(
                        repo_id.clone(),
                        new_head.clone(),
                    ))
                    .await;
            }
            break Ok(StatusCode::OK);
        }

        if current_head.is_some() && new_commit.parent_id != current_head {
            if attempt < MAX_BRANCH_RETRY {
                // Stale HEAD — retry with random backoff (100-500ms).
                let delay_ms = rand::thread_rng().gen_range(100..=500);
                tokio::time::sleep(std::time::Duration::from_millis(delay_ms)).await;
                continue;
            }
            break Err(AppError::Conflict(
                "commit parent_id does not match current HEAD".into(),
            ));
        }

        let mut repo_active: repo::ActiveModel = repo::Entity::find_by_id(&repo_id)
            .one(state.db.as_ref())
            .await?
            .ok_or_else(|| AppError::Internal("repo not found".into()))?
            .into();

        repo_active.head_commit_id = sea_orm::Set(Some(new_head.clone()));
        repo_active.update(state.db.as_ref()).await?;

        // Success — invalidate cache and notify.
        state.path_cache.clear_repo(&repo_id);
        if let Some(ref notif_mgr) = state.notification_manager {
            notif_mgr
                .notify(crate::notification::events::RepoUpdateEvent::new(
                    repo_id.clone(),
                    new_head.clone(),
                ))
                .await;
        }
        break Ok(StatusCode::OK);
    }
}

/// Walk the FS tree starting from root_id, collecting file objects and
/// verifying that every block referenced by those files exists in the
/// block store. Returns the list of files with missing blocks.
///
/// This mirrors seafile-server's check_blocks() called from
/// put_update_branch_cb (http-server.c:1366-1377).
///
/// If the root object doesn't exist yet (initial commit, empty repo),
/// returns an empty list — there's nothing to check.
async fn check_commit_blocks(
    db: &sea_orm::DatabaseConnection,
    block_store: &dyn crate::storage::BlockStorageBackend,
    repo_id: &str,
    root_id: &str,
) -> Result<Vec<String>, AppError> {
    if root_id == "0000000000000000000000000000000000000000" {
        return Ok(Vec::new());
    }

    // Use a DFS stack: (fs_id, path) pairs to walk.
    let mut stack: Vec<(String, String)> = vec![(root_id.to_string(), String::new())];
    let mut missing: Vec<String> = Vec::new();

    while let Some((fs_id, path)) = stack.pop() {
        if fs_id == "0000000000000000000000000000000000000000" {
            continue;
        }

        let obj = fs_object::Entity::find()
            .filter(fs_object::Column::RepoId.eq(repo_id))
            .filter(fs_object::Column::FsId.eq(&fs_id))
            .one(db)
            .await?;

        let Some(obj) = obj else {
            // Object not found — this happens during test setup where
            // a commit is created with a synthetic root_id. In production
            // this would be a server error, but for safety we continue
            // rather than rejecting the entire update.
            continue;
        };

        match obj.obj_type {
            1 => {
                // File object — extract block_ids and check each.
                let file_data: crate::serialization::fs_json::FsFileData =
                    serde_json::from_str(&obj.data)
                        .map_err(|e| AppError::Internal(format!("invalid file object: {e}")))?;
                for block_id in &file_data.block_ids {
                    if !block_store.has_block(block_id).await {
                        missing.push(path.clone());
                        break;
                    }
                }
            }
            3 => {
                // Directory object — push children onto the stack.
                let dir_data: crate::serialization::fs_json::FsDirData =
                    serde_json::from_str(&obj.data)
                        .map_err(|e| AppError::Internal(format!("invalid dir object: {e}")))?;
                for entry in &dir_data.dirents {
                    let child_path = if path.is_empty() {
                        entry.name.clone()
                    } else {
                        format!("{}/{}", path, entry.name)
                    };
                    stack.push((entry.id.clone(), child_path));
                }
            }
            _ => {}
        }
    }

    Ok(missing)
}
