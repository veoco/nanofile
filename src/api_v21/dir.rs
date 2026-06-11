use axum::{
    Json,
    extract::{Path, Query, State},
    http::{HeaderMap, HeaderName, HeaderValue},
    response::{IntoResponse, Response},
};
use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};
use serde::Deserialize;
use std::sync::Arc;

use crate::AppState;
use crate::auth::middleware::AuthUser;
use crate::entity::{commit, repo, repo_member};
use crate::error::AppError;

#[derive(Deserialize)]
pub struct V21DirQuery {
    pub p: Option<String>,
    pub with_thumbnail: Option<bool>,
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
) -> Result<Response, AppError> {
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
    let (dir_id, entries) =
        crate::api::dir::list_dir_from_fs_tree(db, &repo_id, &normalized).await?;

    // Get the user's permission for this repo.
    let user_perm = repo_member::Entity::find()
        .filter(repo_member::Column::RepoId.eq(&repo_id))
        .filter(repo_member::Column::UserId.eq(auth.user_id))
        .one(db)
        .await?
        .map(|m| m.permission)
        .unwrap_or_else(|| "rw".to_string());

    // Separate dirs and files, then sort alphabetically (case-insensitive),
    // matching seahub's DirView behavior.
    let mut dir_list: Vec<_> = Vec::new();
    let mut file_list: Vec<_> = Vec::new();
    for e in entries {
        if e.entry_type == "dir" {
            dir_list.push(e);
        } else {
            file_list.push(e);
        }
    }
    dir_list.sort_by_key(|a| a.name.to_lowercase());
    file_list.sort_by_key(|a| a.name.to_lowercase());

    // Match seahub's normalize_dir_path: non-root directories must have a
    // trailing slash in parent_dir so clients (e.g. Android) can safely
    // construct full_path = parent_dir + name without a missing separator.
    let parent_dir = if normalized == "/" {
        normalized.clone()
    } else {
        format!("{}/", normalized.trim_end_matches('/'))
    };
    let mut dirent_list = Vec::with_capacity(dir_list.len() + file_list.len());

    // Directory entries — no size, no modifier fields.
    for e in &dir_list {
        dirent_list.push(serde_json::json!({
            "type": "dir",
            "id": e.id,
            "name": e.name,
            "mtime": e.mtime,
            "permission": e.permission,
            "parent_dir": parent_dir,
            "starred": false,
        }));
    }

    // File entries — include size, modifier info, and thumbnail info.
    let with_thumbnail = query.with_thumbnail.unwrap_or(false);
    for e in &file_list {
        let modifier_email = e.modifier.as_str();
        // Seahub's email2nickname() returns the local part of the email
        // (the part before '@') when no nickname is configured, not the
        // full email address and not an empty string.
        let modifier_name = modifier_email.split('@').next().unwrap_or("");
        // Seahub's email2contact_email() returns the profile's contact
        // email, or the original login email if none is configured.
        let modifier_contact_email = modifier_email;
        // When with_thumbnail=true, include encoded_thumbnail_src (empty
        // for non-image files) so clients that expect this field don't
        // fail on its absence.  Seahub returns it conditionally for
        // IMAGE/PDF/SVG only; the empty fallback is pragmatically safer
        // for the HarmonyOS client which sends with_thumbnail=true on
        // every directory listing.
        let mut entry = serde_json::json!({
            "type": "file",
            "id": e.id,
            "name": e.name,
            "size": e.size,
            "mtime": e.mtime,
            "permission": e.permission,
            "parent_dir": parent_dir,
            "starred": false,
            "modifier_email": modifier_email,
            "modifier_name": modifier_name,
            "modifier_contact_email": modifier_contact_email,
        });
        if with_thumbnail {
            entry["encoded_thumbnail_src"] = serde_json::Value::String(String::new());
        }
        dirent_list.push(entry);
    }

    // Set the oid HTTP header matching the v2 API behavior (used by iOS client for caching).
    let mut headers = HeaderMap::new();
    if !dir_id.is_empty() {
        headers.insert(
            HeaderName::from_static("oid"),
            HeaderValue::from_str(&dir_id).unwrap(),
        );
    }

    let body = serde_json::json!({
        "user_perm": user_perm,
        "dir_id": dir_id,
        "dirent_list": dirent_list,
    });

    Ok((headers, Json(body)).into_response())
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
/// Or with p in the query string:
/// ```http
/// POST /api/v2.1/repos/{repo_id}/dir/?p=/newdir
/// Content-Type: application/json
/// {"operation": "mkdir"}
/// ```
pub async fn create_dir_v21(
    auth: AuthUser,
    State(state): State<Arc<AppState>>,
    Path(repo_id): Path<String>,
    Query(query): Query<V21DirQuery>,
    Json(body): Json<CreateDirBody>,
) -> Result<Json<serde_json::Value>, AppError> {
    // Permission check
    crate::storage::check_repo_write_permission(state.db.as_ref(), &repo_id, auth.user_id).await?;

    // Prefer p from body, fall back to query string (matching seahub behavior).
    let path = body
        .p
        .or(query.p)
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
