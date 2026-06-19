use axum::{
    Json,
    extract::{Path, Query, State},
    http::{HeaderMap, HeaderName, HeaderValue},
    response::{IntoResponse, Response},
};
use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};
use serde::Deserialize;
use std::collections::HashSet;
use std::sync::Arc;

use crate::AppState;
use crate::activity_log;
use crate::api::dir::DirEntry;
use crate::auth::middleware::AuthUser;
use crate::entity::{commit, repo, repo_member, starred_file, user};
use crate::error::AppError;

#[derive(Deserialize)]
pub struct V21DirQuery {
    pub p: Option<String>,
    pub t: Option<String>,
    pub recursive: Option<String>,
    pub with_thumbnail: Option<bool>,
}

/// DELETE /api/v2.1/repos/{repo_id}/{obj}/?p=path
pub async fn delete_dirent_v21(
    auth: AuthUser,
    State(state): State<Arc<AppState>>,
    Path((repo_id, obj)): Path<(String, String)>,
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
    let parent_fs_id =
        crate::storage::resolve_fs_id(db, &repo_id, &head_commit.root_id, parent_path)
            .await
            .map_err(|e| AppError::Internal(format!("resolve parent failed: {e}")))?;

    // Get entry size before deletion (for repo size adjustment).
    let deleted_size: i64 = crate::storage::get_entry_total_size(db, &repo_id, &normalized)
        .await
        .unwrap_or_default();

    // --- TRASH: Record deleted entry before tree update ---
    // Use `.ok()` to discard `Box<dyn Error>` which is !Send and would break
    // the axum handler trait.
    #[allow(clippy::match_result_ok, clippy::collapsible_if)]
    if let Some(parent_dir_data) = crate::storage::read_fs_dir_data(db, &repo_id, &parent_fs_id)
        .await
        .ok()
    {
        if let Some(entry) = parent_dir_data.dirents.iter().find(|d| d.name == name) {
            let obj_type = if entry.mode & crate::serialization::S_IFDIR != 0 {
                "dir"
            } else {
                "file"
            };
            if let Err(e) = crate::storage::trash::TrashService::add_to_trash(
                db,
                &repo_id,
                &normalized,
                obj_type,
                &entry.id,
                &entry.name,
                entry.size,
                &head_commit_id,
                &auth.email,
            )
            .await
            {
                tracing::warn!("Failed to record trash for {normalized}: {e}");
            }
        }
    }
    // --- END TRASH ---

    // Update the FS tree and create a commit
    crate::storage::file_ops::FileOps::update_dir_tree_and_commit(
        db,
        &repo_id,
        parent_path,
        &parent_fs_id,
        &auth.email,
        &format!("Deleted {}", name),
        crate::storage::file_ops::EMPTY_ANCESTOR_CHAIN,
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

    // Log activity
    activity_log::log_activity(
        db,
        &repo_id,
        "delete",
        &obj,
        &normalized,
        auth.user_id,
        None,
        None,
        None,
        None,
    )
    .await;

    Ok(Json(serde_json::json!({"success": true})))
}

/// Wrapper for delete_dirent_v21 that works with Path(repo_id) instead of
/// Path((repo_id, obj)).  Matches delete_file_v21 in `src/api_v21/file.rs`.
///
/// The Android client sends `DELETE /api/v2.1/repos/{repo_id}/dir/?p=path`,
/// which Axum matches against the literal route `{repo_id}/dir/` (more
/// specific than `{repo_id}/{obj}/`), so we need a dedicated DELETE handler.
pub async fn delete_dir_v21(
    auth: AuthUser,
    State(state): State<Arc<AppState>>,
    repo_id: axum::extract::Path<String>,
    query: axum::extract::Query<V21DirQuery>,
) -> Result<Json<serde_json::Value>, AppError> {
    delete_dirent_v21(
        auth,
        axum::extract::State(state),
        axum::extract::Path((repo_id.0, "dir".to_string())),
        query,
    )
    .await
}

/// GET /api/v2.1/repos/{repo_id}/dir/?p=&t=&recursive=&with_thumbnail=true
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

    // Validate recursive param
    if let Some(ref r) = query.recursive
        && r != "0"
        && r != "1"
    {
        return Err(AppError::BadRequest(
            "If you want to get recursive dir entries, you should set 'recursive' argument as '1'."
                .into(),
        ));
    }
    // Validate t (type filter) param
    if let Some(ref t) = query.t
        && t != "f"
        && t != "d"
    {
        return Err(AppError::BadRequest(
            "'t'(type) should be 'f' or 'd'.".into(),
        ));
    }

    // Recursive listing path
    if query.recursive.as_deref() == Some("1") {
        let (dir_id, all_entries) =
            crate::api::dir::list_dir_recursive_from_fs_tree(db, &repo_id, &normalized).await?;
        let dirent_list: Vec<DirEntry> = match query.t.as_deref() {
            Some("f") => all_entries
                .into_iter()
                .filter(|e| e.entry_type == "file")
                .collect(),
            Some("d") => all_entries
                .into_iter()
                .filter(|e| e.entry_type == "dir")
                .collect(),
            _ => all_entries,
        };

        // Get the user's permission for this repo.
        let user_perm = repo_member::Entity::find()
            .filter(repo_member::Column::RepoId.eq(&repo_id))
            .filter(repo_member::Column::UserId.eq(auth.user_id))
            .one(db)
            .await?
            .map(|m| m.permission)
            .unwrap_or_else(|| "rw".to_string());

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
        return Ok((headers, Json(body)).into_response());
    }

    // Non-recursive path (same as before)
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

    // Query starred entries for this user+repo so we can stamp `starred` on
    // each dirent, matching seahub's behavior (api2/endpoints/dir.py:59-65).
    let starred_set: HashSet<String> = starred_file::Entity::find()
        .filter(starred_file::Column::UserId.eq(auth.user_id))
        .filter(starred_file::Column::RepoId.eq(&repo_id))
        .all(db)
        .await?
        .into_iter()
        .map(|s| s.path.trim_end_matches('/').to_string())
        .collect();

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
        let entry_path = format!("{}{}", parent_dir, e.name);
        dirent_list.push(serde_json::json!({
            "type": "dir",
            "id": e.id,
            "name": e.name,
            "mtime": e.mtime,
            "permission": e.permission,
            "parent_dir": parent_dir,
            "starred": starred_set.contains(&entry_path),
        }));
    }

    // File entries — include size, modifier info, and thumbnail info.
    let with_thumbnail = query.with_thumbnail.unwrap_or(false);

    // Batch-load user nicknames for modifier emails to avoid N+1 queries.
    let mut nickname_cache: std::collections::HashMap<String, String> =
        std::collections::HashMap::new();
    let modifier_emails: std::collections::HashSet<&str> =
        file_list.iter().map(|e| e.modifier.as_str()).collect();
    for email in &modifier_emails {
        if !email.is_empty() {
            let name = user::Entity::find()
                .filter(user::Column::Email.eq(*email))
                .one(db)
                .await?
                .map(|u| u.nickname())
                .unwrap_or_else(|| email.split('@').next().unwrap_or("").to_string());
            nickname_cache.insert((*email).to_string(), name);
        }
    }

    for e in &file_list {
        let modifier_email = e.modifier.as_str();
        let modifier_name = nickname_cache
            .get(modifier_email)
            .map(|s| s.as_str())
            .unwrap_or_else(|| modifier_email.split('@').next().unwrap_or(""));
        // Seahub's email2contact_email() returns the profile's contact
        // email, or the original login email if none is configured.
        let modifier_contact_email = modifier_email;
        // When with_thumbnail=true, include encoded_thumbnail_src (empty
        // for non-image files) so clients that expect this field don't
        // fail on its absence.  Seahub returns it conditionally for
        // IMAGE/PDF/SVG only; the empty fallback is pragmatically safer
        // for the HarmonyOS client which sends with_thumbnail=true on
        // every directory listing.
        let entry_path = format!("{}{}", parent_dir, e.name);
        let mut entry = serde_json::json!({
            "type": "file",
            "id": e.id,
            "name": e.name,
            "size": e.size,
            "mtime": e.mtime,
            "permission": e.permission,
            "parent_dir": parent_dir,
            "starred": starred_set.contains(&entry_path),
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

/// `GET /api/v2.1/repos/{repo_id}/dir/detail/?path=...`
///
/// Returns metadata about a directory (not its contents).
/// The `path` query parameter is required and must not be `/`.
///
/// Response matches seahub's `DirDetailView`:
/// ```json
/// {
///   "repo_id": "...",
///   "path": "...",
///   "name": "...",
///   "mtime": 1234567890,
///   "permission": "rw"
/// }
/// ```
/// Query parameters for dir detail endpoint.
/// Seahub and seadroid send `path`, not `p`.
#[derive(Deserialize)]
pub struct DirDetailQuery {
    pub path: Option<String>,
}

pub async fn dir_detail_v21(
    auth: AuthUser,
    State(state): State<Arc<AppState>>,
    Path(repo_id): Path<String>,
    Query(query): Query<DirDetailQuery>,
) -> Result<Json<serde_json::Value>, AppError> {
    // Permission check — read access is sufficient.
    crate::storage::check_repo_read_permission(state.db.as_ref(), &repo_id, auth.user_id).await?;

    // path is required and must not be "/"
    let path = query
        .path
        .ok_or_else(|| AppError::BadRequest("path required".into()))?;
    if path == "/" || path.is_empty() {
        return Err(AppError::BadRequest("path invalid.".into()));
    }
    let normalized = if path.starts_with('/') {
        path.to_string()
    } else {
        format!("/{}", path)
    };

    // Resolve the directory's fs_id via the FS tree.
    let db = state.db.as_ref();
    let repo_record = crate::entity::repo::Entity::find_by_id(&repo_id)
        .one(db)
        .await?
        .ok_or_else(|| AppError::NotFound("Library not found".into()))?;
    let head_commit_id = repo_record
        .head_commit_id
        .ok_or_else(|| AppError::NotFound("no commits".into()))?;
    let head_commit = crate::entity::commit::Entity::find()
        .filter(crate::entity::commit::Column::CommitId.eq(&head_commit_id))
        .one(db)
        .await?
        .ok_or_else(|| AppError::NotFound("head commit not found".into()))?;

    crate::storage::resolve_fs_id(db, &repo_id, &head_commit.root_id, &normalized)
        .await
        .map_err(|_| AppError::NotFound("Folder not found.".into()))?;

    // Read the directory data to get entry metadata files in the parent listing.
    // The dir name is the last component of the path.
    let dir_name = normalized
        .trim_end_matches('/')
        .rsplit_once('/')
        .map(|(_, n)| n)
        .unwrap_or("");

    // Look up the entry in the parent directory for its mtime.
    let parent_path = match normalized.trim_end_matches('/').rsplit_once('/') {
        Some(("", _)) => "/",
        Some((parent, _)) => parent,
        None => "/",
    };
    let mtime = if parent_path == "/" {
        // Root children — resolve the root dir
        let root_data = crate::storage::read_fs_dir_data(db, &repo_id, &head_commit.root_id)
            .await
            .unwrap_or_else(|_| crate::serialization::fs_json::FsDirData {
                dirents: vec![],
                obj_type: crate::serialization::fs_json::SEAF_METADATA_TYPE_DIR,
                version: 1,
            });
        root_data
            .dirents
            .iter()
            .find(|d| d.name == dir_name)
            .map(|d| d.mtime)
            .unwrap_or(0)
    } else {
        let parent_fs_id =
            match crate::storage::resolve_fs_id(db, &repo_id, &head_commit.root_id, parent_path)
                .await
            {
                Ok(id) => id,
                Err(_) => return Err(AppError::NotFound("Folder not found.".into())),
            };
        let parent_data = crate::storage::read_fs_dir_data(db, &repo_id, &parent_fs_id)
            .await
            .map_err(|e| AppError::Internal(format!("read parent failed: {e}")))?;
        parent_data
            .dirents
            .iter()
            .find(|d| d.name == dir_name)
            .map(|d| d.mtime)
            .unwrap_or(0)
    };

    // Get user permission
    let permission = crate::entity::repo_member::Entity::find()
        .filter(crate::entity::repo_member::Column::RepoId.eq(&repo_id))
        .filter(crate::entity::repo_member::Column::UserId.eq(auth.user_id))
        .one(db)
        .await?
        .map(|m| m.permission)
        .unwrap_or_else(|| "rw".to_string());

    Ok(Json(serde_json::json!({
        "repo_id": repo_id,
        "path": normalized,
        "name": dir_name,
        "mtime": mtime,
        "permission": permission,
    })))
}
