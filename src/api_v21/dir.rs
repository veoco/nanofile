use axum::{
    Json,
    extract::{Path, Query, State},
};
use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

use crate::AppState;
use crate::auth::middleware::AuthUser;
use crate::entity::{commit, repo};
use crate::error::AppError;

#[derive(Deserialize)]
pub struct V21DirQuery {
    pub p: Option<String>,
    pub with_thumbnail: Option<bool>,
}

#[derive(Serialize)]
pub struct V21DirListResponse {
    pub dirent_list: Vec<serde_json::Value>,
}

/// DELETE /api/v2.1/repos/{repo_id}/{obj}/?p=path
pub async fn delete_dirent_v21(
    auth: AuthUser,
    State(state): State<Arc<AppState>>,
    Path((repo_id, _obj)): Path<(String, String)>,
    Query(query): Query<V21DirQuery>,
) -> Result<Json<serde_json::Value>, AppError> {
    // Permission check
    crate::storage::check_repo_write_permission(state.db.as_ref(), &repo_id, auth.user_id).await?;

    let path = query
        .p
        .as_deref()
        .ok_or_else(|| AppError::BadRequest("path required".into()))?;
    let normalized = if path.starts_with('/') {
        path.to_string()
    } else {
        format!("/{}", path)
    };

    let db = state.db.as_ref();

    let name = normalized.rsplit_once('/').map(|(_, n)| n).unwrap_or("");
    let parent_path = match normalized.rsplit_once('/') {
        Some(("", _)) => "/",
        Some((parent, _)) => parent,
        None => "/",
    };

    // Get root fs_id from head commit
    let repo_model = repo::Entity::find_by_id(&repo_id)
        .one(db)
        .await?
        .ok_or_else(|| AppError::NotFound("repo not found".into()))?;
    let head_commit_id = repo_model
        .head_commit_id
        .ok_or_else(|| AppError::NotFound("no commits".into()))?;
    let head_commit = commit::Entity::find()
        .filter(commit::Column::CommitId.eq(&head_commit_id))
        .one(db)
        .await?
        .ok_or_else(|| AppError::NotFound("head commit not found".into()))?;

    // Resolve parent's current fs_id
    let parent_fs_id = crate::storage::resolve_fs_id(
        db,
        &repo_id,
        &head_commit.root_id,
        parent_path,
        Some(state.path_cache.as_ref()),
    )
    .await
    .map_err(|e| AppError::Internal(format!("resolve parent failed: {e}")))?;

    // Get entry size before deletion (for repo size adjustment).
    let deleted_size: i64 = crate::storage::get_entry_total_size(db, &repo_id, &normalized)
        .await
        .unwrap_or_default();

    // Update the FS tree and create a commit
    crate::storage::file_ops::FileOps::update_dir_tree_and_commit(
        db,
        &repo_id,
        parent_path,
        &parent_fs_id,
        &auth.email,
        &format!("Deleted {}", name),
        Some(state.path_cache.as_ref()),
        |dirents| {
            dirents.retain(|d| d.name != name);
            Ok(())
        },
    )
    .await
    .map_err(|e| AppError::Internal(e.to_string()))?;

    // Remove from full-text search index.
    if let Some(indexer) = &state.indexer
        && let Err(e) = indexer.delete_file(&repo_id, &normalized)
    {
        tracing::warn!("Failed to delete index for {normalized}: {e}");
    }

    // Adjust repo size (subtract the deleted entry's size).
    crate::storage::adjust_repo_size(db, &repo_id, -deleted_size).await?;

    Ok(Json(serde_json::json!({"success": true})))
}

/// GET /api/v2.1/repos/{repo_id}/dir/?p=&with_thumbnail=true
pub async fn list_dir_v21(
    auth: AuthUser,
    State(state): State<Arc<AppState>>,
    Path(repo_id): Path<String>,
    Query(query): Query<V21DirQuery>,
) -> Result<Json<V21DirListResponse>, AppError> {
    // Permission check
    crate::storage::check_repo_read_permission(state.db.as_ref(), &repo_id, auth.user_id).await?;

    let path = query.p.as_deref().unwrap_or("/");
    let normalized = if path.starts_with('/') {
        path.to_string()
    } else {
        format!("/{}", path)
    };
    let db = state.db.as_ref();

    // Always list from the FS object tree, which is the authoritative source.
    let (_, entries) = crate::api::dir::list_dir_from_fs_tree(db, &repo_id, &normalized).await?;

    Ok(Json(V21DirListResponse {
        dirent_list: entries
            .into_iter()
            .map(|e| {
                serde_json::json!({
                    "name": e.name,
                    "type": e.entry_type,
                    "size": e.size,
                    "last_modified": e.mtime,
                    "id": e.id,
                })
            })
            .collect(),
    }))
}

#[derive(Deserialize)]
pub struct CreateDirBody {
    p: Option<String>,
    /// Accepted for API compatibility (desktop client sends it), but not used.
    #[serde(rename = "operation")]
    _operation: Option<String>,
}

/// POST /api/v2.1/repos/{repo_id}/dir/
///
/// Create a directory. Accepts JSON body:
/// ```json
/// {"p": "/newdir", "operation": "mkdir"}
/// ```
/// Or just:
/// ```json
/// {"p": "/newdir"}
/// ```
pub async fn create_dir_v21(
    auth: AuthUser,
    State(state): State<Arc<AppState>>,
    Path(repo_id): Path<String>,
    Json(body): Json<CreateDirBody>,
) -> Result<Json<serde_json::Value>, AppError> {
    // Permission check
    crate::storage::check_repo_write_permission(state.db.as_ref(), &repo_id, auth.user_id).await?;

    let path = body
        .p
        .ok_or_else(|| AppError::BadRequest("path (p) required".into()))?;
    let path = if path.starts_with('/') {
        path
    } else {
        format!("/{}", path)
    };

    // Delegate to the v2 API's create_dir_by_path
    crate::api::dir::create_dir_by_path(auth, state, repo_id, path).await?;

    Ok(Json(serde_json::json!({"success": true})))
}
