use axum::http::HeaderMap;
use axum::{
    Json, Router,
    body::Body,
    extract::{Path, Query, State},
    http::Request,
    response::IntoResponse,
};
use sea_orm::{ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;

use crate::AppState;
use crate::activity_log;
use crate::api::repos::extract_multipart_field;
use crate::auth::middleware::AuthUser;
use crate::entity::{commit, fs_object, repo, repo_member, share_link};
use crate::error::AppError;
use crate::serialization::fs_json::{DirEntryData, FsDirData, SEAF_METADATA_TYPE_DIR};
use crate::storage::file_ops::FileOps;

/// Extract the parent directory path from a full path.
/// `/dir/file.txt` → `/dir`,  `/file.txt` → `/`
fn parent_path_from(path: &str) -> &str {
    match path.rsplit_once('/') {
        Some(("", _)) => "/",
        Some((parent, _)) => parent,
        None => "/",
    }
}

#[derive(Deserialize)]
pub struct DirQuery {
    pub p: Option<String>,
    /// Type filter: "f" (files only) or "d" (dirs only). Used with recursive=1.
    pub t: Option<String>,
    /// "1" to list entries recursively (flat list). "0" or absent for single-level listing.
    pub recursive: Option<String>,
}

/// Ensure path starts with "/" for consistent DB lookups.
fn normalize_path(path: &str) -> String {
    if path.is_empty() || path == "/" {
        "/".to_string()
    } else if path.starts_with('/') {
        path.to_string()
    } else {
        format!("/{}", path)
    }
}

#[derive(Serialize)]
pub struct DirEntry {
    pub id: String,
    #[serde(rename = "type")]
    pub entry_type: String,
    pub name: String,
    pub size: i64,
    pub mtime: i64,
    pub permission: String,
    /// Last modifier email (empty string if unknown). Files only in the
    /// original seafile protocol, but we store it for all entry types.
    #[serde(default)]
    pub modifier: String,
    /// Parent directory path. Present only in recursive listings.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_dir: Option<String>,
    /// Modifier display name. Present only for file entries in recursive listings.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub modifier_name: Option<String>,
    /// Modifier contact email. Present only for file entries in recursive listings.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub modifier_contact_email: Option<String>,
}

/// Get the root_fs_id from the repo's head commit for path resolution.
async fn get_head_root_id(db: &DatabaseConnection, repo_id: &str) -> Result<String, AppError> {
    let repo_record = repo::Entity::find_by_id(repo_id)
        .one(db)
        .await?
        .ok_or_else(|| AppError::NotFound("Repository not found".to_string()))?;
    let head_commit_id = repo_record
        .head_commit_id
        .ok_or_else(|| AppError::NotFound("No commits yet".to_string()))?;
    let head = commit::Entity::find()
        .filter(commit::Column::RepoId.eq(repo_id))
        .filter(commit::Column::CommitId.eq(&head_commit_id))
        .one(db)
        .await?
        .ok_or_else(|| AppError::NotFound("Head commit not found".to_string()))?;
    Ok(head.root_id)
}

pub fn dir_routes() -> Router<Arc<AppState>> {
    Router::new()
        .route(
            "/{repo_id}/dir/",
            axum::routing::get(list_dir)
                .post(dir_post_handler)
                .delete(delete_dir),
        )
        // Keep dedicated rename/move endpoints for JSON-speaking callers
        .route("/{repo_id}/dir/move/", axum::routing::post(move_dir))
        .route("/{repo_id}/dir/rename/", axum::routing::post(rename_dir))
        .route(
            "/{repo_id}/dir/shared_items/",
            axum::routing::get(dir_shared_items),
        )
        .route(
            "/{repo_id}/dir/sub_repo/",
            axum::routing::get(create_sub_repo),
        )
}

pub async fn list_dir(
    auth: AuthUser,
    State(state): State<Arc<AppState>>,
    Path(repo_id): Path<String>,
    Query(query): Query<DirQuery>,
) -> Result<impl IntoResponse, AppError> {
    // Permission check
    crate::storage::check_repo_read_permission(state.db.as_ref(), &repo_id, auth.user_id).await?;

    let path = normalize_path(&query.p.unwrap_or_else(|| "/".to_string()));
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
        let (dir_id, all_entries) = list_dir_recursive_from_fs_tree(db, &repo_id, &path).await?;
        let filtered: Vec<DirEntry> = match query.t.as_deref() {
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
        let mut headers = HeaderMap::new();
        if !dir_id.is_empty() {
            headers.insert("oid", dir_id.parse().unwrap());
        }
        headers.insert("dir_perm", "rw".parse().unwrap());
        return Ok((headers, Json(filtered)));
    }

    // Non-recursive path (unchanged)
    let (dir_id, entries) = list_dir_from_fs_tree(db, &repo_id, &path).await?;
    let mut headers = HeaderMap::new();
    if !dir_id.is_empty() {
        headers.insert("oid", dir_id.parse().unwrap());
    }
    headers.insert("dir_perm", "rw".parse().unwrap());
    Ok((headers, Json(entries)))
}

/// List directory entries by traversing the FS object tree from the head commit.
/// This is the authoritative source (dir_entries table has been removed).
/// Returns `(dir_id, entries)` where `dir_id` is the SHA-1 fs_id of the listed directory.
pub(crate) async fn list_dir_from_fs_tree(
    db: &DatabaseConnection,
    repo_id: &str,
    path: &str,
) -> Result<(String, Vec<DirEntry>), AppError> {
    // Get the repo to find the head commit
    let repo_record = repo::Entity::find_by_id(repo_id)
        .one(db)
        .await?
        .ok_or_else(|| AppError::NotFound("Repository not found".into()))?;

    let head_commit_id = match repo_record.head_commit_id {
        Some(id) => id,
        None => return Ok((String::new(), vec![])), // Empty repo
    };

    // Get the head commit to find the root fs_id
    let head = commit::Entity::find()
        .filter(commit::Column::RepoId.eq(repo_id))
        .filter(commit::Column::CommitId.eq(&head_commit_id))
        .one(db)
        .await?
        .ok_or_else(|| AppError::NotFound("Head commit not found".into()))?;

    // Traverse the path to find the target directory's fs_id
    let dir_id = crate::storage::resolve_fs_id(db, repo_id, &head.root_id, path)
        .await
        .map_err(|e| AppError::internal(format!("resolve_fs_id failed: {e}")))?;

    // Read the directory data
    let dir_data = read_fs_dir_data(db, repo_id, &dir_id).await?;

    Ok((
        dir_id,
        dir_data
            .dirents
            .into_iter()
            .map(|d| DirEntry {
                id: d.id,
                entry_type: if d.mode & crate::serialization::S_IFDIR != 0 {
                    "dir".to_string()
                } else {
                    "file".to_string()
                },
                name: d.name,
                size: d.size,
                mtime: d.mtime,
                permission: "rw".to_string(),
                modifier: d.modifier,
                parent_dir: None,
                modifier_name: None,
                modifier_contact_email: None,
            })
            .collect(),
    ))
}

/// Read and parse an FsDirData object from the fs_objects table.
async fn read_fs_dir_data(
    db: &DatabaseConnection,
    repo_id: &str,
    fs_id: &str,
) -> Result<FsDirData, AppError> {
    crate::storage::read_fs_dir_data(db, repo_id, fs_id)
        .await
        .map_err(|e| AppError::internal(format!("read fs_dir_data failed: {e}")))
}

/// Recursively list all directory entries from the FS object tree.
///
/// Uses an iterative stack (same pattern as `search_fs_tree` in search.rs)
/// to avoid deep recursion. Returns a flat list of all entries at every
/// depth, each with its `parent_dir` set to the containing directory path.
pub(crate) async fn list_dir_recursive_from_fs_tree(
    db: &DatabaseConnection,
    repo_id: &str,
    path: &str,
) -> Result<(String, Vec<DirEntry>), AppError> {
    // Resolve the target directory's fs_id
    let repo_record = repo::Entity::find_by_id(repo_id)
        .one(db)
        .await?
        .ok_or_else(|| AppError::NotFound("Repository not found".into()))?;

    let head_commit_id = match repo_record.head_commit_id {
        Some(id) => id,
        None => return Ok((String::new(), vec![])),
    };

    let head = commit::Entity::find()
        .filter(commit::Column::RepoId.eq(repo_id))
        .filter(commit::Column::CommitId.eq(&head_commit_id))
        .one(db)
        .await?
        .ok_or_else(|| AppError::NotFound("Head commit not found".into()))?;

    let dir_id = crate::storage::resolve_fs_id(db, repo_id, &head.root_id, path)
        .await
        .map_err(|e| AppError::internal(format!("resolve_fs_id failed: {e}")))?;

    // Iterative stack: (fs_id, parent_path)
    let mut stack: Vec<(String, String)> = vec![(dir_id.clone(), path.to_string())];
    let mut entries: Vec<DirEntry> = Vec::new();

    while let Some((fs_id, parent_path)) = stack.pop() {
        // EMPTY_SHA1 sentinel
        if fs_id == "0000000000000000000000000000000000000000" {
            continue;
        }

        let dir_data = match crate::storage::read_fs_dir_data(db, repo_id, &fs_id).await {
            Ok(d) => d,
            Err(_) => continue, // skip unreadable objects
        };

        for dirent in &dir_data.dirents {
            let is_dir = dirent.mode & crate::serialization::S_IFDIR != 0;
            let modifier_email = dirent.modifier.clone();

            let mut entry = DirEntry {
                id: dirent.id.clone(),
                entry_type: if is_dir {
                    "dir".to_string()
                } else {
                    "file".to_string()
                },
                name: dirent.name.clone(),
                size: dirent.size,
                mtime: dirent.mtime,
                permission: "rw".to_string(),
                modifier: modifier_email.clone(),
                parent_dir: Some(parent_path.clone()),
                modifier_name: None,
                modifier_contact_email: None,
            };

            // For files, derive modifier_name and modifier_contact_email from the modifier email.
            if !is_dir && !modifier_email.is_empty() {
                let local = modifier_email.split('@').next().unwrap_or("");
                entry.modifier_name = Some(local.to_string());
                entry.modifier_contact_email = Some(modifier_email);
            }

            entries.push(entry);

            // Push subdirectories onto the stack
            if is_dir {
                let child_path = if parent_path == "/" {
                    format!("/{}", dirent.name)
                } else {
                    format!("{}/{}", parent_path, dirent.name)
                };
                stack.push((dirent.id.clone(), child_path));
            }
        }
    }

    Ok((dir_id, entries))
}

/// Combined POST handler for `/api2/repos/{id}/dir/`.
///
/// Seafile clients send the directory path in the query parameter `p`
/// and the operation (if any) in the request body — as JSON, form data,
/// or even without a body at all.  To maximise compatibility the handler:
///
/// 1. Always tries JSON parsing first (Content-Type is not reliable).
/// 2. Falls back to form-urlencoded parsing on JSON failure.
/// 3. If neither yields an operation, defaults to `"mkdir"`.
///
/// | Format | Body | `p` source |
/// |--------|------|------------|
/// | JSON | `{"operation":"mkdir"}` | query (or body) |
/// | JSON | `{"p":"/path"}` | body |
/// | Form | `operation=mkdir` | query |
/// | Empty | — | query (implicit mkdir) |
pub async fn dir_post_handler(
    auth: AuthUser,
    State(state): State<Arc<AppState>>,
    Path(repo_id): Path<String>,
    Query(query): Query<DirQuery>,
    req: Request<Body>,
) -> Result<Json<serde_json::Value>, AppError> {
    // Permission check
    crate::storage::check_repo_write_permission(state.db.as_ref(), &repo_id, auth.user_id).await?;

    let (_parts, body) = req.into_parts();
    let bytes = axum::body::to_bytes(body, usize::MAX)
        .await
        .map_err(|e| AppError::Internal(e.to_string()))?;

    // Extract (operation, p, newname) from whatever the client sent.
    // Try JSON first, then form-urlencoded.
    let (op, p, newname): (Option<String>, Option<String>, Option<String>) =
        if let Ok(json_val) = serde_json::from_slice::<serde_json::Value>(&bytes) {
            let op = json_val
                .get("operation")
                .and_then(|v| v.as_str())
                .map(String::from);
            let p = json_val.get("p").and_then(|v| v.as_str()).map(String::from);
            let newname = json_val
                .get("newname")
                .and_then(|v| v.as_str())
                .map(String::from);
            (op, p, newname)
        } else if let Ok(form) = serde_urlencoded::from_bytes::<HashMap<String, String>>(&bytes) {
            let op = form.get("operation").cloned();
            let p = form.get("p").cloned();
            let newname = form.get("newname").cloned();
            (op, p, newname)
        } else {
            // Try multipart/form-data raw-text scan (Android client sends rename
            // via @Multipart @PartMap with operation=rename, newname=xxx).
            let op = extract_multipart_field(&bytes, "operation");
            let p = extract_multipart_field(&bytes, "p");
            let newname = extract_multipart_field(&bytes, "newname");
            (op, p, newname)
        };

    // p: body first, query second.
    let path = p
        .or_else(|| query.p.clone())
        .ok_or_else(|| AppError::BadRequest("path required".into()))?;
    let path = normalize_path(&path);

    // Operation defaults to "mkdir" when unspecified.
    match op.as_deref() {
        Some("rename") => {
            let newname = newname.ok_or_else(|| AppError::BadRequest("newname required".into()))?;
            rename_dir_entry(
                state.db.as_ref(),
                &repo_id,
                &path,
                &newname,
                &auth.email,
                auth.user_id,
            )
            .await?;
            // Return JSON string "success" (not a JSON object) so the Android
            // client's SupportResponseConverter can parse it for Call<String>.
            Ok(Json(serde_json::Value::String("success".to_string())))
        }
        _ => {
            // mkdir (default when operation is missing, "mkdir", or unknown).
            // NOTE: Return JSON string "success" (not a JSON object with dir
            // info) because the Android client's SupportResponseConverter uses
            // TypeAdapter<String>.fromJson() for ALL Call<String> and
            // Single<String> responses, and TypeAdapter<String> throws on a
            // JSON object. The full dir info is available via the v2.1
            // create_dir_v21 endpoint for clients that need it.
            create_dir_by_path(auth, state, repo_id.clone(), path).await?;
            Ok(Json(serde_json::Value::String("success".to_string())))
        }
    }
}

/// Shared directory rename logic (uses FS tree, no dir_entry).
async fn rename_dir_entry(
    db: &DatabaseConnection,
    repo_id: &str,
    path: &str,
    new_name: &str,
    modifier: &str,
    user_id: i32,
) -> Result<(), AppError> {
    let parent_path = parent_path_from(path);
    let old_name = path.rsplit_once('/').map(|(_, n)| n).unwrap_or("");

    // Resolve parent's current fs_id via the FS tree
    let head_root_id = get_head_root_id(db, repo_id).await?;
    let parent_fs_id = crate::storage::resolve_fs_id(db, repo_id, &head_root_id, parent_path)
        .await
        .map_err(|e| AppError::Internal(format!("resolve parent failed: {e}")))?;

    // Read parent's FsDirData to find the child's fs_id
    let parent_data = crate::storage::read_fs_dir_data(db, repo_id, &parent_fs_id)
        .await
        .map_err(|e| AppError::Internal(format!("read parent failed: {e}")))?;
    let child_id = parent_data
        .dirents
        .iter()
        .find(|d| d.name == old_name)
        .map(|d| d.id.clone())
        .ok_or_else(|| AppError::NotFound("directory not found".into()))?;

    // Update the FS tree and create a commit
    // Match by child_id (fs_id) for robustness.
    FileOps::update_dir_tree_and_commit(
        db,
        repo_id,
        parent_path,
        &parent_fs_id,
        modifier,
        &format!("Renamed directory {}", old_name),
        crate::storage::file_ops::EMPTY_ANCESTOR_CHAIN,
        |dirents| {
            if let Some(d) = dirents.iter_mut().find(|d| d.id == child_id) {
                d.name = new_name.to_string();
            }
            Ok(())
        },
    )
    .await
    .map_err(|e| AppError::Internal(e.to_string()))?;

    // Log activity
    let new_path = if parent_path == "/" {
        format!("/{}", new_name)
    } else {
        format!("{}/{}", parent_path, new_name)
    };
    activity_log::log_activity(db, repo_id, "rename", "dir", &new_path, user_id, Some(path)).await;

    Ok(())
}

/// Create a directory at the given path.
pub(crate) async fn create_dir_by_path(
    auth: AuthUser,
    state: Arc<AppState>,
    repo_id: String,
    path: String,
) -> Result<(), AppError> {
    let parts: Vec<&str> = path.trim_start_matches('/').split('/').collect();
    if parts.is_empty() {
        return Err(AppError::BadRequest("invalid path".into()));
    }

    let dir_name = parts.last().unwrap();
    let parent_path = if parts.len() > 1 {
        format!("/{}", parts[..parts.len() - 1].join("/"))
    } else {
        "/".to_string()
    };

    // Use EMPTY_SHA1 sentinel for empty directories.
    // The seafile protocol uses all-zeros ("0000000000000000000000000000000000000000")
    // as a well-known sentinel meaning "empty directory". The C client's diff engine
    // (expand_dir_added_cb in repo-mgr.c) specifically checks for this sentinel to
    // generate DIR_ADDED entries — using a real SHA1 would silently drop the entry
    // during diff expansion and the directory would never be created locally.
    let dir_fs_id = "0000000000000000000000000000000000000000".to_string();
    // No fs_object record needed — EMPTY_SHA1 is a well-known sentinel handled by
    // read_fs_dir_data() and the seafile client natively.

    let now = chrono::Utc::now().timestamp();

    // Find parent directory's fs_id via the head commit's FS tree
    let parent_fs_id = if parent_path == "/" {
        match get_head_root_id(state.db.as_ref(), &repo_id).await {
            Ok(root_id) => root_id,
            Err(_) => {
                // Empty repo — create root fs_object
                let empty_root = FsDirData {
                    dirents: vec![],
                    obj_type: SEAF_METADATA_TYPE_DIR,
                    version: 1,
                };

                empty_root
                    .compute_and_store(state.db.as_ref(), &repo_id)
                    .await?
            }
        }
    } else {
        let head_root_id = get_head_root_id(state.db.as_ref(), &repo_id).await?;
        crate::storage::resolve_fs_id(state.db.as_ref(), &repo_id, &head_root_id, &parent_path)
            .await
            .map_err(|e| AppError::Internal(format!("resolve parent failed: {e}")))?
    };

    // Use update_dir_tree_and_commit to add the new directory entry to parent
    FileOps::update_dir_tree_and_commit(
        state.db.as_ref(),
        &repo_id,
        &parent_path,
        &parent_fs_id,
        &auth.email,
        &format!("Created directory {}", dir_name),
        crate::storage::file_ops::EMPTY_ANCESTOR_CHAIN,
        |dirents| {
            if !dirents.iter().any(|d| d.name == *dir_name) {
                dirents.push(DirEntryData {
                    id: dir_fs_id.clone(),
                    mode: crate::serialization::S_IFDIR,
                    modifier: auth.email.clone(),
                    mtime: now,
                    name: dir_name.to_string(),
                    size: 0,
                });
            }
            Ok(())
        },
    )
    .await
    .map_err(|e| AppError::Internal(e.to_string()))?;

    // Log activity
    activity_log::log_activity(
        state.db.as_ref(),
        &repo_id,
        "create",
        "dir",
        &path,
        auth.user_id,
        None,
    )
    .await;

    Ok(())
}

pub async fn delete_dir(
    auth: AuthUser,
    State(state): State<Arc<AppState>>,
    Path(repo_id): Path<String>,
    Query(query): Query<DirQuery>,
) -> Result<(), AppError> {
    // Permission check
    crate::storage::check_repo_write_permission(state.db.as_ref(), &repo_id, auth.user_id).await?;

    let path = normalize_path(
        &query
            .p
            .ok_or_else(|| AppError::BadRequest("path is required".into()))?,
    );

    let db = state.db.as_ref();
    let name = path.rsplit_once('/').map(|(_, n)| n).unwrap_or("");
    let parent_path = parent_path_from(&path);

    // Get directory total size before deletion (for repo size adjustment).
    let deleted_size = crate::storage::get_entry_total_size(db, &repo_id, &path)
        .await
        .ok()
        .unwrap_or(0);

    // Resolve parent's current fs_id via the FS tree
    let head_root_id = get_head_root_id(db, &repo_id).await?;
    let parent_fs_id = crate::storage::resolve_fs_id(db, &repo_id, &head_root_id, parent_path)
        .await
        .map_err(|e| AppError::Internal(format!("resolve parent failed: {e}")))?;

    // Remove from parent FsDirData and create a commit
    FileOps::update_dir_tree_and_commit(
        db,
        &repo_id,
        parent_path,
        &parent_fs_id,
        &auth.email,
        &format!("Deleted directory {}", name),
        crate::storage::file_ops::EMPTY_ANCESTOR_CHAIN,
        |dirents| {
            dirents.retain(|d| d.name != name);
            Ok(())
        },
    )
    .await
    .map_err(|e| AppError::Internal(e.to_string()))?;

    // Adjust repo size (subtract the deleted directory's total size).
    crate::storage::adjust_repo_size(db, &repo_id, -deleted_size).await?;

    // Log activity
    activity_log::log_activity(db, &repo_id, "delete", "dir", &path, auth.user_id, None).await;

    Ok(())
}

#[derive(Deserialize)]
pub struct MoveDirRequest {
    pub repo_id: String,
    pub p: String,
    pub new_parent_dir: String,
}

pub async fn move_dir(
    auth: AuthUser,
    State(state): State<Arc<AppState>>,
    Json(req): Json<MoveDirRequest>,
) -> Result<(), AppError> {
    // Permission check
    crate::storage::check_repo_write_permission(state.db.as_ref(), &req.repo_id, auth.user_id)
        .await?;

    let db = state.db.as_ref();

    // Resolve head commit root for path lookups
    let head_root_id = get_head_root_id(db, &req.repo_id).await?;

    let parent_path = parent_path_from(&req.p);
    let dir_name = req.p.rsplit_once('/').map(|(_, n)| n).unwrap_or("");

    // Get old parent's current fs_id
    let old_parent_fs_id =
        crate::storage::resolve_fs_id(db, &req.repo_id, &head_root_id, parent_path)
            .await
            .map_err(|e| AppError::Internal(format!("resolve old parent failed: {e}")))?;

    // Read old parent's FsDirData to find the directory entry's metadata
    let old_parent_data = crate::storage::read_fs_dir_data(db, &req.repo_id, &old_parent_fs_id)
        .await
        .map_err(|e| AppError::Internal(format!("read old parent failed: {e}")))?;
    let entry = old_parent_data
        .dirents
        .iter()
        .find(|d| d.name == dir_name)
        .ok_or_else(|| AppError::NotFound("directory not found".into()))?;

    let dir_fs_id = entry.id.clone();
    let dir_mode = entry.mode;
    let dir_size = entry.size;

    let new_parent_path = normalize_path(&req.new_parent_dir);
    let _new_parent_fs_id =
        crate::storage::resolve_fs_id(db, &req.repo_id, &head_root_id, &new_parent_path)
            .await
            .map_err(|e| AppError::Internal(format!("resolve dest parent failed: {e}")))?;

    // Step 1: Remove from old parent's FsDirData, create commit
    let intermediate_root = FileOps::update_dir_tree_no_commit(
        db,
        &req.repo_id,
        parent_path,
        &old_parent_fs_id,
        crate::storage::file_ops::EMPTY_ANCESTOR_CHAIN,
        |dirents| {
            dirents.retain(|d| d.name != dir_name);
            Ok(())
        },
    )
    .await
    .map_err(|e| AppError::Internal(e.to_string()))?;

    FileOps::create_commit(
        db,
        &req.repo_id,
        &intermediate_root,
        &auth.email,
        &format!("Moved directory {}", dir_name),
    )
    .await
    .map_err(|e| AppError::Internal(e.to_string()))?;

    // Step 2: Re-read head, resolve destination, add entry with commit
    let new_head_root = get_head_root_id(db, &req.repo_id).await?;
    let new_dst_fs_id =
        crate::storage::resolve_fs_id(db, &req.repo_id, &new_head_root, &new_parent_path)
            .await
            .map_err(|e| {
                AppError::Internal(format!("resolve dest dir after removal failed: {e}"))
            })?;

    let now = chrono::Utc::now().timestamp();
    FileOps::update_dir_tree_and_commit(
        db,
        &req.repo_id,
        &new_parent_path,
        &new_dst_fs_id,
        &auth.email,
        &format!("Moved directory {}", dir_name),
        crate::storage::file_ops::EMPTY_ANCESTOR_CHAIN,
        |dirents| {
            if !dirents.iter().any(|d| d.name == dir_name) {
                dirents.push(DirEntryData {
                    id: dir_fs_id.clone(),
                    mode: dir_mode,
                    modifier: auth.email.clone(),
                    mtime: now,
                    name: dir_name.to_string(),
                    size: dir_size,
                });
            }
            Ok(())
        },
    )
    .await
    .map_err(|e| AppError::Internal(e.to_string()))?;

    // Log activity
    let new_path = if new_parent_path == "/" {
        format!("/{}", dir_name)
    } else {
        format!("{}/{}", new_parent_path, dir_name)
    };
    activity_log::log_activity(
        db,
        &req.repo_id,
        "move",
        "dir",
        &new_path,
        auth.user_id,
        Some(&req.p),
    )
    .await;

    Ok(())
}

#[derive(Deserialize)]
pub struct RenameDirRequest {
    pub repo_id: String,
    pub p: String,
    pub new_name: String,
}

pub async fn rename_dir(
    auth: AuthUser,
    State(state): State<Arc<AppState>>,
    Json(req): Json<RenameDirRequest>,
) -> Result<(), AppError> {
    // Permission check
    crate::storage::check_repo_write_permission(state.db.as_ref(), &req.repo_id, auth.user_id)
        .await?;

    let path = normalize_path(&req.p);
    rename_dir_entry(
        state.db.as_ref(),
        &req.repo_id,
        &path,
        &req.new_name,
        &auth.email,
        auth.user_id,
    )
    .await
}

#[derive(Serialize)]
pub struct DirSharedItemsResponse {
    pub shared_items: Vec<serde_json::Value>,
}

/// `GET /api2/repos/{repo_id}/dir/shared_items/?p=`
///
/// Returns sharing info for a directory.
pub async fn dir_shared_items(
    auth: AuthUser,
    State(state): State<Arc<AppState>>,
    Path(repo_id): Path<String>,
    Query(query): Query<DirQuery>,
) -> Result<Json<DirSharedItemsResponse>, AppError> {
    crate::storage::check_repo_read_permission(state.db.as_ref(), &repo_id, auth.user_id).await?;

    let path = normalize_path(&query.p.unwrap_or_else(|| "/".to_string()));

    let links = share_link::Entity::find()
        .filter(share_link::Column::RepoId.eq(&repo_id))
        .filter(share_link::Column::Path.eq(&path))
        .all(state.db.as_ref())
        .await?;

    let shared_items: Vec<serde_json::Value> = links
        .into_iter()
        .map(|l| {
            serde_json::json!({
                "share_type": "download",
                "token": l.token,
                "path": l.path,
                "repo_id": l.repo_id,
                "creator_email": "",
                "created_at": l.created_at,
            })
        })
        .collect();

    Ok(Json(DirSharedItemsResponse { shared_items }))
}

#[derive(Serialize)]
pub struct SubRepoResponse {
    pub id: String,
    pub name: String,
    pub desc: String,
    pub size: i64,
    pub encrypted: i32,
    pub enc_version: i32,
    pub owner: String,
    pub permission: String,
    pub mtime: i64,
}

/// Copy all reachable fs_objects from one repo to another, starting from a root fs_id.
/// Uses an iterative stack to avoid recursion.
async fn copy_fs_tree(
    db: &DatabaseConnection,
    src_repo_id: &str,
    dst_repo_id: &str,
    root_fs_id: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut stack = vec![root_fs_id.to_string()];
    while let Some(fs_id) = stack.pop() {
        // EMPTY_SHA1 is a well-known sentinel for empty directories — no fs_object
        // record exists for it and none needs to be created.
        if fs_id == "0000000000000000000000000000000000000000" {
            continue;
        }
        let obj = fs_object::Entity::find()
            .filter(fs_object::Column::RepoId.eq(src_repo_id))
            .filter(fs_object::Column::FsId.eq(&fs_id))
            .one(db)
            .await?
            .ok_or_else(|| format!("fs_object not found: {fs_id}"))?;

        let exists = fs_object::Entity::find()
            .filter(fs_object::Column::RepoId.eq(dst_repo_id))
            .filter(fs_object::Column::FsId.eq(&fs_id))
            .one(db)
            .await?
            .is_some();
        if !exists {
            fs_object::Entity::insert(fs_object::ActiveModel {
                id: sea_orm::NotSet,
                repo_id: sea_orm::Set(dst_repo_id.to_string()),
                fs_id: sea_orm::Set(fs_id.clone()),
                obj_type: sea_orm::Set(obj.obj_type),
                data: sea_orm::Set(obj.data.clone()),
            })
            .exec(db)
            .await?;
        }

        // If directory, push children onto the stack
        if obj.obj_type == SEAF_METADATA_TYPE_DIR as i8 {
            let dir_data: FsDirData = serde_json::from_str(&obj.data)?;
            for entry in &dir_data.dirents {
                stack.push(entry.id.clone());
            }
        }
    }
    Ok(())
}

/// `GET /api2/repos/{repo_id}/dir/sub_repo/?p=/path`
///
/// Creates a new repository from an existing directory's contents.
pub async fn create_sub_repo(
    auth: AuthUser,
    State(state): State<Arc<AppState>>,
    Path(repo_id): Path<String>,
    Query(query): Query<DirQuery>,
) -> Result<Json<SubRepoResponse>, AppError> {
    // Permission check
    crate::storage::check_repo_write_permission(state.db.as_ref(), &repo_id, auth.user_id).await?;

    let path = normalize_path(
        &query
            .p
            .ok_or_else(|| AppError::BadRequest("path required".into()))?,
    );

    // Verify the source directory exists by resolving its path via the FS tree
    let head_root_id = get_head_root_id(state.db.as_ref(), &repo_id).await?;
    let source_dir_fs_id =
        crate::storage::resolve_fs_id(state.db.as_ref(), &repo_id, &head_root_id, &path)
            .await
            .map_err(|_| AppError::NotFound("directory not found".into()))?;

    // Create a new repo
    let new_repo_id = uuid::Uuid::new_v4().to_string();
    let now = chrono::Utc::now().timestamp();
    let dir_name = path
        .trim_matches('/')
        .rsplit('/')
        .next()
        .unwrap_or("subrepo");

    let model = repo::ActiveModel {
        id: sea_orm::Set(new_repo_id.clone()),
        name: sea_orm::Set(dir_name.to_string()),
        description: sea_orm::Set(String::new()),
        owner_id: sea_orm::Set(auth.user_id),
        encrypted: sea_orm::Set(0i8),
        enc_version: sea_orm::Set(0i8),
        magic: sea_orm::Set(None),
        random_key: sea_orm::Set(None),
        salt: sea_orm::Set(String::new()),
        head_commit_id: sea_orm::NotSet,
        permission: sea_orm::Set("rw".to_string()),
        repo_version: sea_orm::Set(1),
        size: sea_orm::Set(0),
        created_at: sea_orm::Set(now),
        updated_at: sea_orm::Set(now),
    };
    repo::Entity::insert(model).exec(state.db.as_ref()).await?;

    // Add the user as repo member
    repo_member::Entity::insert(repo_member::ActiveModel {
        id: sea_orm::NotSet,
        repo_id: sea_orm::Set(new_repo_id.clone()),
        user_id: sea_orm::Set(auth.user_id),
        permission: sea_orm::Set("rw".to_string()),
        created_at: sea_orm::Set(now),
    })
    .exec(state.db.as_ref())
    .await?;

    // Copy all fs_objects reachable from the source directory into the new repo
    copy_fs_tree(state.db.as_ref(), &repo_id, &new_repo_id, &source_dir_fs_id)
        .await
        .map_err(|e| AppError::Internal(format!("copy fs tree failed: {e}")))?;

    // Create the initial commit for the new repo, pointing to the source dir's root
    FileOps::create_commit(
        state.db.as_ref(),
        &new_repo_id,
        &source_dir_fs_id,
        &auth.email,
        "Created sub-repo",
    )
    .await
    .map_err(|e| AppError::Internal(format!("create commit failed: {e}")))?;

    Ok(Json(SubRepoResponse {
        id: new_repo_id,
        name: dir_name.to_string(),
        desc: String::new(),
        size: 0,
        encrypted: 0,
        enc_version: 0,
        owner: auth.email,
        permission: "rw".to_string(),
        mtime: now,
    }))
}
