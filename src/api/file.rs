use crate::api::repos::extract_multipart_field;
use axum::{
    Json, Router,
    body::Body,
    extract::{FromRequest, Multipart, Path, Query, State},
    http::{HeaderMap, HeaderName, HeaderValue, Request},
    response::{IntoResponse, Response},
};
use sea_orm::{ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;

use crate::AppState;
use crate::activity_log;
use crate::auth::middleware::AuthUser;
use crate::entity::{commit, fs_object, locked_file, repo, user};
use crate::error::AppError;
use crate::notification::events::FileLockEvent;
use crate::serialization::fs_json::{DirEntryData, SEAF_METADATA_TYPE_DIR};
use crate::storage::file_ops::FileOps;

/// Extract the parent directory path from a full path.
///
/// `/dir/file.txt` → `/dir`
/// `/file.txt` → `/`  (root level)
fn parent_path_from(path: &str) -> &str {
    match path.rsplit_once('/') {
        Some(("", _)) => "/", // root-level: "/file" → ""
        Some((parent, _)) => parent,
        None => "/",
    }
}

#[derive(Deserialize)]
pub struct FileQuery {
    pub p: Option<String>,
    pub reuse: Option<i32>,
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

/// Build a download API URL from the Host header (similar to build_op_url for uploads).
fn build_download_url(state: &AppState, token: &str, host_header: Option<&str>) -> String {
    let (host, port) = if let Some(h) = host_header {
        if let Some((h, p)) = h.split_once(':') {
            (h.to_string(), p.to_string())
        } else {
            (h.to_string(), state.config.server.port.to_string())
        }
    } else if state.config.server.addr == "0.0.0.0"
        || state.config.server.addr == "::"
        || state.config.server.addr == "127.0.0.1"
    {
        (
            "127.0.0.1".to_string(),
            state.config.server.port.to_string(),
        )
    } else {
        (
            state.config.server.addr.clone(),
            state.config.server.port.to_string(),
        )
    };
    format!("http://{}:{}/download-api/{}", host, port, token)
}

/// Build a block download URL in `/blks/{token}/{file_id}/{block_id}` format.
fn build_block_download_url(
    state: &AppState,
    token: &str,
    file_id: &str,
    block_id: &str,
    host_header: Option<&str>,
) -> String {
    let (host, port) = if let Some(h) = host_header {
        if let Some((h, p)) = h.split_once(':') {
            (h.to_string(), p.to_string())
        } else {
            (h.to_string(), state.config.server.port.to_string())
        }
    } else if state.config.server.addr == "0.0.0.0"
        || state.config.server.addr == "::"
        || state.config.server.addr == "127.0.0.1"
    {
        (
            "127.0.0.1".to_string(),
            state.config.server.port.to_string(),
        )
    } else {
        (
            state.config.server.addr.clone(),
            state.config.server.port.to_string(),
        )
    };
    format!(
        "http://{}:{}/blks/{}/{}/{}",
        host, port, token, file_id, block_id
    )
}

/// `GET /api2/repos/{repo_id}/files/{file_id}/blks/{block_id}/download-link/?p=/parent_dir`
///
/// Returns a JSON string URL pointing to the block content, matching seahub's
/// `FileBlockDownloadLinkView`.  The returned URL goes through the `/blks/`
/// handler (step B) which reads the block from the block store.
///
/// The `file_id` parameter is the file's fs_id (SHA1).  The `p` query parameter
/// (optional, default `/`) is used for permission checking.
pub async fn get_block_download_link(
    auth: AuthUser,
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path((repo_id, file_id, block_id)): Path<(String, String, String)>,
    Query(query): Query<HashMap<String, String>>,
) -> Result<Json<String>, AppError> {
    // Permission check using the parent_dir from query (p).
    let parent_dir = query.get("p").map(|s| s.as_str()).unwrap_or("/");
    crate::storage::check_repo_read_permission(state.db.as_ref(), &repo_id, auth.user_id).await?;

    // Generate a downloadblks token for auth validation.
    let token = state.token_manager.generate(
        &repo_id,
        auth.user_id,
        &auth.email,
        "downloadblks",
        parent_dir,
    );

    let host_header = headers.get("host").and_then(|v| v.to_str().ok());
    let url = build_block_download_url(&state, &token, &file_id, &block_id, host_header);
    Ok(Json(url))
}

/// Get the root_fs_id from the repo's head commit for path resolution.
pub(crate) async fn get_head_root_id(
    db: &DatabaseConnection,
    repo_id: &str,
) -> Result<String, AppError> {
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

pub fn file_routes() -> Router<Arc<AppState>> {
    Router::new()
        .route(
            "/{repo_id}/file/",
            axum::routing::get(download_file)
                .post(file_post_handler)
                .put(lock_file_via_api_handler)
                .delete(delete_file),
        )
        // Keep /rename/ and /move/ for JSON-speaking callers
        .route("/{repo_id}/file/rename/", axum::routing::post(rename_file))
        .route("/{repo_id}/file/move/", axum::routing::post(move_file))
        .route("/{repo_id}/file/detail/", axum::routing::get(file_detail))
}

pub async fn download_file(
    auth: AuthUser,
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(repo_id): Path<String>,
    Query(query): Query<FileQuery>,
) -> Result<Response, AppError> {
    // Permission check
    crate::storage::check_repo_read_permission(state.db.as_ref(), &repo_id, auth.user_id).await?;

    let path = normalize_path(&query.p.unwrap_or_else(|| "/".to_string()));
    let db = state.db.as_ref();

    // Resolve the file's fs_id from the FS tree (same as file_detail handler).
    let head_root_id = get_head_root_id(db, &repo_id).await?;
    let file_fs_id = crate::storage::resolve_fs_id(db, &repo_id, &head_root_id, &path)
        .await
        .map_err(|_| AppError::NotFound("file not found".into()))?;

    // Extract filename from path.
    let filename = path.rsplit_once('/').map(|(_, n)| n).unwrap_or("download");

    // Generate a download token with file metadata.
    let download_token = state.token_manager.generate_download(
        &repo_id,
        auth.user_id,
        &auth.email,
        &path,
        &file_fs_id,
        filename,
    );

    // Build the download URL using the Host header (same approach as upload-link).
    let host_header = headers.get("host").and_then(|v| v.to_str().ok());
    let url = build_download_url(&state, &download_token, host_header);

    // Return oid header + JSON-quoted URL string matching the seadroid client's expectation.
    let mut resp_headers = HeaderMap::new();
    resp_headers.insert(
        HeaderName::from_static("oid"),
        HeaderValue::from_str(&file_fs_id).unwrap(),
    );

    Ok((resp_headers, Json(url)).into_response())
}

/// Combined POST handler for `/api2/repos/{id}/file/`:
/// - Multipart form-data → file upload
/// - Form-urlencoded `operation=rename&newname=xxx` → rename
/// - Form-urlencoded `operation=move&dst_repo=...&dst_dir=...` → move
pub async fn file_post_handler(
    auth: AuthUser,
    State(state): State<Arc<AppState>>,
    Path(repo_id): Path<String>,
    Query(query): Query<FileQuery>,
    req: Request<Body>,
) -> Result<Json<serde_json::Value>, AppError> {
    // Permission check
    crate::storage::check_repo_write_permission(state.db.as_ref(), &repo_id, auth.user_id).await?;

    let (parts, body) = req.into_parts();
    let bytes = axum::body::to_bytes(body, usize::MAX)
        .await
        .map_err(|e| AppError::Internal(e.to_string()))?;

    let content_type = parts
        .headers
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();

    if content_type.starts_with("multipart/form-data") {
        // Check if this is a rename operation from Android client (multipart
        // with operation=rename+newname).  Scan the raw bytes to avoid
        // consuming the stream before we know what to do.
        if let Some(op) = extract_multipart_field(&bytes, "operation")
            && op == "rename"
        {
            let newname = extract_multipart_field(&bytes, "newname")
                .ok_or_else(|| AppError::BadRequest("newname required".into()))?;
            let path = normalize_path(&query.p.unwrap_or_default());
            rename_file_entry(
                state.db.as_ref(),
                &repo_id,
                &path,
                &newname,
                &auth.email,
                auth.user_id,
            )
            .await?;
            Ok(Json(serde_json::Value::String("success".to_string())))
        } else {
            // File upload — reconstruct for Multipart extractor
            let body = Body::from(bytes);
            let req = Request::from_parts(parts, body);
            let mut multipart = Multipart::from_request(req, &state)
                .await
                .map_err(|e| AppError::Internal(e.to_string()))?;
            upload_file_inner(auth, state, repo_id, &mut multipart).await
        }
    } else {
        // Form-encoded operations: rename, move
        let form: HashMap<String, String> = serde_urlencoded::from_bytes(&bytes)
            .map_err(|_| AppError::BadRequest("invalid form data".into()))?;
        let path = normalize_path(&query.p.unwrap_or_default());

        match form.get("operation").map(|s| s.as_str()) {
            Some("rename") => {
                let newname = form
                    .get("newname")
                    .ok_or_else(|| AppError::BadRequest("newname required".into()))?;
                rename_file_entry(
                    state.db.as_ref(),
                    &repo_id,
                    &path,
                    newname,
                    &auth.email,
                    auth.user_id,
                )
                .await?;
                // Update full-text search index: delete old path, re-index new path.
                if let Some(indexer) = &state.indexer {
                    let new_fullpath = if path == "/" || path.is_empty() {
                        format!("/{}", newname)
                    } else {
                        let parent = parent_path_from(&path);
                        format!("{}/{}", parent, newname)
                    };
                    if let Err(e) = indexer.delete_file(&repo_id, &path) {
                        tracing::warn!("Failed to delete old index on rename: {e}");
                    }
                    if let Err(e) = indexer
                        .reindex_file(
                            state.db.as_ref(),
                            &repo_id,
                            &new_fullpath,
                            &state.block_store,
                        )
                        .await
                    {
                        tracing::warn!("Failed to reindex renamed file: {e}");
                    }
                }
                Ok(Json(serde_json::json!({"success": true})))
            }
            Some("move") => {
                let dst_repo = form
                    .get("dst_repo")
                    .ok_or_else(|| AppError::BadRequest("dst_repo required".into()))?;
                let dst_dir = form.get("dst_dir").map(|s| s.as_str()).unwrap_or("/");
                move_file_entry(
                    state.db.as_ref(),
                    &repo_id,
                    &path,
                    dst_repo,
                    dst_dir,
                    &auth.email,
                    auth.user_id,
                )
                .await?;
                // Update full-text search index: delete old path, re-index at new path.
                if let Some(indexer) = &state.indexer {
                    let file_name = path.rsplit_once('/').map(|(_, n)| n).unwrap_or("");
                    let new_fullpath = if dst_dir == "/" {
                        format!("/{}", file_name)
                    } else {
                        format!("{}/{}", dst_dir, file_name)
                    };
                    if let Err(e) = indexer.delete_file(&repo_id, &path) {
                        tracing::warn!("Failed to delete old index on move: {e}");
                    }
                    if let Err(e) = indexer
                        .reindex_file(
                            state.db.as_ref(),
                            &repo_id,
                            &new_fullpath,
                            &state.block_store,
                        )
                        .await
                    {
                        tracing::warn!("Failed to reindex moved file: {e}");
                    }
                }
                Ok(Json(serde_json::json!({"success": true})))
            }
            _ => Err(AppError::BadRequest("unknown operation".into())),
        }
    }
}

/// Multipart file upload handler (shared by file_post_handler).
async fn upload_file_inner(
    auth: AuthUser,
    state: Arc<AppState>,
    repo_id: String,
    multipart: &mut Multipart,
) -> Result<Json<serde_json::Value>, AppError> {
    let mut file_data = Vec::new();
    let mut file_name = String::new();
    let mut parent_dir = "/".to_string();
    let mut replace = false;

    while let Some(field) = multipart
        .next_field()
        .await
        .map_err(|e| AppError::Internal(e.to_string()))?
    {
        let name = field.name().unwrap_or_default().to_string();
        match name.as_str() {
            "file" => {
                file_name = field.file_name().unwrap_or_default().to_string();
                file_data = field
                    .bytes()
                    .await
                    .map_err(|e| AppError::Internal(e.to_string()))?
                    .to_vec();
            }
            "parent_dir" => {
                parent_dir = String::from_utf8(
                    field
                        .bytes()
                        .await
                        .map_err(|e| AppError::Internal(e.to_string()))?
                        .to_vec(),
                )
                .unwrap_or_default();
            }
            "replace" => {
                let val = String::from_utf8(
                    field
                        .bytes()
                        .await
                        .map_err(|e| AppError::Internal(e.to_string()))?
                        .to_vec(),
                )
                .unwrap_or_default();
                replace = val == "1" || val == "true";
            }
            _ => {}
        }
    }

    if file_name.is_empty() {
        return Err(AppError::BadRequest("no file provided".into()));
    }

    // Get old file size for incremental repo size adjustment.
    let file_path = if parent_dir == "/" {
        format!("/{}", file_name)
    } else {
        format!("{}/{}", parent_dir, file_name)
    };
    let old_size = if replace {
        crate::storage::get_entry_total_size(state.db.as_ref(), &repo_id, &file_path)
            .await
            .ok()
            .unwrap_or(0)
    } else {
        0
    };

    FileOps::create_file(
        state.db.as_ref(),
        &repo_id,
        &parent_dir,
        &file_name,
        &file_data,
        &auth.email,
        replace,
        &state.block_store,
        None,
    )
    .await
    .map_err(|e| AppError::Internal(e.to_string()))?;

    // Log activity
    let op_type = if replace { "edit" } else { "create" };
    activity_log::log_activity(
        state.db.as_ref(),
        &repo_id,
        op_type,
        "file",
        &file_path,
        auth.user_id,
        None,
    )
    .await;

    // Adjust repo size (delta = new_size - old_size).
    crate::storage::adjust_repo_size(
        state.db.as_ref(),
        &repo_id,
        file_data.len() as i64 - old_size,
    )
    .await?;

    // Index text file content for full-text search.
    if let Some(indexer) = &state.indexer {
        let full_path = if parent_dir.ends_with('/') {
            format!("{}{}", parent_dir, file_name)
        } else if parent_dir == "/" {
            format!("/{}", file_name)
        } else {
            format!("{}/{}", parent_dir, file_name)
        };
        if crate::indexer::is_indexable_text(&file_name, &file_data) {
            let content = String::from_utf8_lossy(&file_data);
            if let Err(e) = indexer.index_file(&repo_id, &full_path, &file_name, &content) {
                tracing::warn!("Failed to index file {file_name}: {e}");
            }
        } else if replace {
            // Binary file replaced — clean up any previous text index.
            if let Err(e) = indexer.delete_file(&repo_id, &full_path) {
                tracing::warn!("Failed to delete index for {file_name}: {e}");
            }
        }
    }

    Ok(Json(serde_json::json!({"success": true})))
}

pub async fn delete_file(
    auth: AuthUser,
    State(state): State<Arc<AppState>>,
    Path(repo_id): Path<String>,
    Query(query): Query<FileQuery>,
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

    // Get entry size before deletion (for repo size adjustment).
    let deleted_size = crate::storage::get_entry_total_size(db, &repo_id, &path)
        .await
        .ok()
        .unwrap_or(0);

    // Get root fs_id from head commit and resolve parent directory
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
        && let Err(e) = indexer.delete_file(&repo_id, &path)
    {
        tracing::warn!("Failed to delete index for {path}: {e}");
    }

    // Adjust repo size (subtract the deleted entry's size).
    crate::storage::adjust_repo_size(db, &repo_id, -deleted_size).await?;

    // Log activity
    activity_log::log_activity(db, &repo_id, "delete", "file", &path, auth.user_id, None).await;

    Ok(())
}

#[derive(Deserialize)]
pub struct MoveRequest {
    pub repo_id: String,
    pub p: String,
    pub new_parent_dir: String,
}

pub async fn move_file(
    auth: AuthUser,
    State(state): State<Arc<AppState>>,
    Json(req): Json<MoveRequest>,
) -> Result<(), AppError> {
    // Permission check
    crate::storage::check_repo_write_permission(state.db.as_ref(), &req.repo_id, auth.user_id)
        .await?;

    let path = normalize_path(&req.p);
    move_file_entry(
        state.db.as_ref(),
        &req.repo_id,
        &path,
        &req.new_parent_dir,
        &req.new_parent_dir,
        &auth.email,
        auth.user_id,
    )
    .await?;

    // Update full-text search index: delete old path, re-index at new path.
    if let Some(indexer) = &state.indexer {
        let file_name = path.rsplit_once('/').map(|(_, n)| n).unwrap_or("");
        let new_fullpath = if req.new_parent_dir == "/" {
            format!("/{}", file_name)
        } else {
            format!("{}/{}", req.new_parent_dir, file_name)
        };
        if let Err(e) = indexer.delete_file(&req.repo_id, &path) {
            tracing::warn!("Failed to delete old index on move: {e}");
        }
        if let Err(e) = indexer
            .reindex_file(
                state.db.as_ref(),
                &req.repo_id,
                &new_fullpath,
                &state.block_store,
            )
            .await
        {
            tracing::warn!("Failed to reindex moved file: {e}");
        }
    }

    Ok(())
}

/// Shared file rename logic used by both form-encoded and JSON handlers.
pub(crate) async fn rename_file_entry(
    db: &DatabaseConnection,
    repo_id: &str,
    path: &str,
    new_name: &str,
    modifier: &str,
    user_id: i32,
) -> Result<(), AppError> {
    let parent_path = parent_path_from(path);
    let old_name = path.rsplit_once('/').map(|(_, n)| n).unwrap_or("");

    // Get root fs_id from head commit and resolve parent directory
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
        .ok_or_else(|| AppError::NotFound("file not found".into()))?;

    // Update the FS tree and create a commit
    // Match by child_id (fs_id) for robustness.
    FileOps::update_dir_tree_and_commit(
        db,
        repo_id,
        parent_path,
        &parent_fs_id,
        modifier,
        &format!("Renamed {}", old_name),
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
    activity_log::log_activity(
        db,
        repo_id,
        "rename",
        "file",
        &new_path,
        user_id,
        Some(path),
    )
    .await;

    Ok(())
}

/// Shared file move logic used by both form-encoded and JSON handlers.
///
/// Uses a two-commit approach to ensure correctness:
/// 1. Remove from old parent, create a commit
/// 2. Add to new parent, create a second commit
///
/// A single-commit approach would lose the removal because the second
/// `update_dir_tree_no_commit` walks up from the original HEAD, not from
#[allow(clippy::too_many_arguments)]
async fn move_file_entry(
    db: &DatabaseConnection,
    repo_id: &str,
    path: &str,
    _dst_repo: &str,
    dst_dir: &str,
    modifier: &str,
    user_id: i32,
) -> Result<(), AppError> {
    // Get root fs_id from head commit
    let head_root_id = get_head_root_id(db, repo_id).await?;

    let file_name = path.rsplit_once('/').map(|(_, n)| n).unwrap_or("");
    let parent_path = parent_path_from(path);

    // Get OLD parent's current fs_id via FS tree resolution
    let old_parent_fs_id = crate::storage::resolve_fs_id(db, repo_id, &head_root_id, parent_path)
        .await
        .map_err(|e| AppError::Internal(format!("resolve old parent failed: {e}")))?;

    // Read old parent's FsDirData to find the file's metadata
    let old_parent_data = crate::storage::read_fs_dir_data(db, repo_id, &old_parent_fs_id)
        .await
        .map_err(|e| AppError::Internal(format!("read old parent failed: {e}")))?;
    let file_entry = old_parent_data
        .dirents
        .iter()
        .find(|d| d.name == file_name)
        .ok_or_else(|| AppError::NotFound("file not found".into()))?;

    let file_fs_id = file_entry.id.clone();
    let file_mode = file_entry.mode;
    let file_size = file_entry.size;

    // Find destination directory's current fs_id
    let new_parent_path = normalize_path(dst_dir);
    let _new_parent_fs_id =
        crate::storage::resolve_fs_id(db, repo_id, &head_root_id, &new_parent_path)
            .await
            .map_err(|e| AppError::Internal(format!("resolve dest parent failed: {e}")))?;

    // Step 1: Remove from old parent, create commit
    let intermediate_root = FileOps::update_dir_tree_no_commit(
        db,
        repo_id,
        parent_path,
        &old_parent_fs_id,
        crate::storage::file_ops::EMPTY_ANCESTOR_CHAIN,
        |dirents| {
            dirents.retain(|d| d.name != file_name);
            Ok(())
        },
    )
    .await
    .map_err(|e| AppError::Internal(e.to_string()))?;

    FileOps::create_commit(
        db,
        repo_id,
        &intermediate_root,
        modifier,
        &format!("Moved {}", file_name),
    )
    .await
    .map_err(|e| AppError::Internal(e.to_string()))?;

    // Step 2: Re-read head, resolve destination in new tree, add entry
    let new_head_root = get_head_root_id(db, repo_id).await?;
    let new_dst_fs_id =
        crate::storage::resolve_fs_id(db, repo_id, &new_head_root, &new_parent_path)
            .await
            .map_err(|e| {
                AppError::Internal(format!("resolve dest dir after removal failed: {e}"))
            })?;

    let now = chrono::Utc::now().timestamp();
    FileOps::update_dir_tree_and_commit(
        db,
        repo_id,
        &new_parent_path,
        &new_dst_fs_id,
        modifier,
        &format!("Moved {}", file_name),
        crate::storage::file_ops::EMPTY_ANCESTOR_CHAIN,
        |dirents| {
            // Only add if not already present at destination.
            // seafile-server skips the move silently on collision.
            if !dirents.iter().any(|d| d.name == file_name) {
                dirents.push(DirEntryData {
                    id: file_fs_id.clone(),
                    mode: file_mode,
                    modifier: modifier.to_string(),
                    mtime: now,
                    name: file_name.to_string(),
                    size: file_size,
                });
            }
            Ok(())
        },
    )
    .await
    .map_err(|e| AppError::Internal(e.to_string()))?;

    // Log activity
    let new_path = if new_parent_path == "/" {
        format!("/{}", file_name)
    } else {
        format!("{}/{}", new_parent_path, file_name)
    };
    activity_log::log_activity(db, repo_id, "move", "file", &new_path, user_id, Some(path)).await;

    Ok(())
}

#[derive(Deserialize)]
pub struct RenameRequest {
    pub repo_id: String,
    pub p: String,
    pub new_name: String,
}

pub async fn rename_file(
    auth: AuthUser,
    State(state): State<Arc<AppState>>,
    Json(req): Json<RenameRequest>,
) -> Result<(), AppError> {
    // Permission check
    crate::storage::check_repo_write_permission(state.db.as_ref(), &req.repo_id, auth.user_id)
        .await?;

    let path = normalize_path(&req.p);
    rename_file_entry(
        state.db.as_ref(),
        &req.repo_id,
        &path,
        &req.new_name,
        &auth.email,
        auth.user_id,
    )
    .await?;

    // Update full-text search index.
    if let Some(indexer) = &state.indexer {
        let new_fullpath = if path == "/" || path.is_empty() {
            format!("/{}", req.new_name)
        } else {
            let parent = parent_path_from(&path);
            format!("{}/{}", parent, req.new_name)
        };
        if let Err(e) = indexer.delete_file(&req.repo_id, &path) {
            tracing::warn!("Failed to delete old index on rename: {e}");
        }
        if let Err(e) = indexer
            .reindex_file(
                state.db.as_ref(),
                &req.repo_id,
                &new_fullpath,
                &state.block_store,
            )
            .await
        {
            tracing::warn!("Failed to reindex renamed file: {e}");
        }
    }

    Ok(())
}

#[derive(Serialize)]
pub struct FileDetailResponse {
    pub id: String,
    #[serde(rename = "type")]
    pub obj_type: String,
    pub name: String,
    pub size: i64,
    pub last_modified: i64,
    pub last_modifier_name: String,
    pub last_modifier_email: String,
}

/// `GET /api2/repos/{repo_id}/file/detail/?p=/path`
///
/// Returns file metadata including size, modification time, and last modifier.
pub async fn file_detail(
    auth: AuthUser,
    State(state): State<Arc<AppState>>,
    Path(repo_id): Path<String>,
    Query(query): Query<FileQuery>,
) -> Result<Json<FileDetailResponse>, AppError> {
    // Permission check
    crate::storage::check_repo_read_permission(state.db.as_ref(), &repo_id, auth.user_id).await?;

    let path = normalize_path(
        &query
            .p
            .ok_or_else(|| AppError::BadRequest("path is required".into()))?,
    );

    let db = state.db.as_ref();

    // Resolve the file path via the FS tree to get the file's fs_id
    let head_root_id = get_head_root_id(db, &repo_id).await?;
    let file_fs_id = crate::storage::resolve_fs_id(db, &repo_id, &head_root_id, &path)
        .await
        .map_err(|_| AppError::NotFound("file not found".into()))?;

    // Verify it is a file (not a directory).
    // EMPTY_SHA1 is the sentinel for empty directories — no fs_object record exists.
    // A directory can also have a real fs_object with obj_type == DIR.
    if file_fs_id == "0000000000000000000000000000000000000000" {
        return Err(AppError::BadRequest(
            "path is a directory, not a file".into(),
        ));
    }
    let file_obj = fs_object::Entity::find()
        .filter(fs_object::Column::RepoId.eq(&repo_id))
        .filter(fs_object::Column::FsId.eq(&file_fs_id))
        .one(db)
        .await?
        .ok_or_else(|| AppError::NotFound("file not found".into()))?;

    if file_obj.obj_type == SEAF_METADATA_TYPE_DIR as i8 {
        return Err(AppError::BadRequest(
            "path is a directory, not a file".into(),
        ));
    }

    // Get parent directory data for the entry's metadata (name, modifier, mtime)
    let parent_path = parent_path_from(&path);
    let file_name = path.rsplit_once('/').map(|(_, n)| n).unwrap_or("");

    let parent_fs_id = crate::storage::resolve_fs_id(db, &repo_id, &head_root_id, parent_path)
        .await
        .map_err(|e| AppError::Internal(format!("resolve parent failed: {e}")))?;
    let parent_data = crate::storage::read_fs_dir_data(db, &repo_id, &parent_fs_id)
        .await
        .map_err(|e| AppError::Internal(format!("read parent failed: {e}")))?;
    let entry = parent_data
        .dirents
        .iter()
        .find(|e| e.name == file_name)
        .ok_or_else(|| AppError::NotFound("file not found in parent".into()))?;

    // Look up the modifier's email from the users table
    let modifier_email = user::Entity::find()
        .filter(user::Column::Email.eq(&entry.modifier))
        .one(db)
        .await?
        .map(|u| u.email)
        .unwrap_or_else(|| entry.modifier.clone());

    Ok(Json(FileDetailResponse {
        id: file_fs_id,
        obj_type: "file".to_string(),
        name: entry.name.clone(),
        size: entry.size,
        last_modified: entry.mtime,
        last_modifier_name: entry.modifier.clone(),
        last_modifier_email: modifier_email,
    }))
}

/// PUT /api2/repos/{repo_id}/file/?p=path
///
/// Handles file lock/unlock operations for the desktop client's REST API.
/// Form-encoded body: `operation=lock` or `operation=unlock`
pub async fn lock_file_via_api_handler(
    auth: AuthUser,
    State(state): State<Arc<AppState>>,
    Path(repo_id): Path<String>,
    Query(query): Query<FileQuery>,
    req: Request<Body>,
) -> Result<Json<serde_json::Value>, AppError> {
    let (_parts, body) = req.into_parts();
    let bytes = axum::body::to_bytes(body, usize::MAX)
        .await
        .map_err(|e| AppError::Internal(e.to_string()))?;

    let form: HashMap<String, String> = serde_urlencoded::from_bytes(&bytes)
        .map_err(|_| AppError::BadRequest("invalid form data".into()))?;

    let operation = form
        .get("operation")
        .map(|s| s.as_str())
        .ok_or_else(|| AppError::BadRequest("operation required".into()))?;

    let path = normalize_path(&query.p.unwrap_or_default());

    let db = state.db.as_ref();

    // Permission check: only users with write access can lock/unlock files.
    crate::storage::check_repo_write_permission(db, &repo_id, auth.user_id).await?;

    // Look up user ID from email
    let user_record = user::Entity::find()
        .filter(user::Column::Email.eq(&auth.email))
        .one(db)
        .await?
        .ok_or_else(|| AppError::NotFound("user not found".into()))?;

    match operation {
        "lock" => {
            use sea_orm::Set;
            // Check if already locked
            let existing = locked_file::Entity::find()
                .filter(locked_file::Column::RepoId.eq(&repo_id))
                .filter(locked_file::Column::Path.eq(&path))
                .one(db)
                .await?;

            if existing.is_none() {
                locked_file::Entity::insert(locked_file::ActiveModel {
                    id: sea_orm::NotSet,
                    repo_id: Set(repo_id.clone()),
                    path: Set(path.clone()),
                    user_id: Set(user_record.id),
                    locked_at: Set(chrono::Utc::now().timestamp()),
                    lock_owner_name: Set(auth.email.clone()),
                })
                .exec(db)
                .await?;
            }

            // Send file-lock-changed notification to WebSocket subscribers.
            if let Some(mgr) = &state.notification_manager {
                let event = FileLockEvent {
                    repo_id: repo_id.clone(),
                    path: path.clone(),
                    change_event: "locked".to_string(),
                    lock_user: auth.email.clone(),
                };
                mgr.notify(event).await;
            }

            Ok(Json(serde_json::json!({"success": true})))
        }
        "unlock" => {
            locked_file::Entity::delete_many()
                .filter(locked_file::Column::RepoId.eq(&repo_id))
                .filter(locked_file::Column::Path.eq(&path))
                .exec(db)
                .await?;

            // Send file-lock-changed notification to WebSocket subscribers.
            if let Some(mgr) = &state.notification_manager {
                let event = FileLockEvent {
                    repo_id: repo_id.clone(),
                    path: path.clone(),
                    change_event: "unlocked".to_string(),
                    lock_user: auth.email.clone(),
                };
                mgr.notify(event).await;
            }

            Ok(Json(serde_json::json!({"success": true})))
        }
        _ => Err(AppError::BadRequest(format!(
            "unknown operation: {}",
            operation
        ))),
    }
}
