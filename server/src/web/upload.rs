use axum::{
    Json,
    extract::{Multipart, Path, State},
    http::HeaderMap,
};
use serde_json::json;
use std::collections::HashMap;
use std::sync::Arc;

use crate::AppState;
use crate::activity_log;
use crate::entity::repo;
use crate::error::AppError;
use crate::repo::file_ops::FileOps;
use crate::sharing::service::link as upload_link_service;
use crate::ui::auth_extractor::WebUser;
use sea_orm::EntityTrait;

// ─── Helpers ─────────────────────────────────────────────────────────────────

/// Get encryption key for an encrypted repo if the user has set a password.
#[allow(dead_code)]
async fn get_encryption_key_for_repo(
    state: &AppState,
    repo_id: &str,
    user_id: i32,
) -> Result<Option<(Vec<u8>, Vec<u8>)>, AppError> {
    let repo_model = repo::Entity::find_by_id(repo_id)
        .one(state.db.as_ref())
        .await?
        .ok_or_else(|| AppError::NotFound("repo not found".into()))?;

    if repo_model.encrypted == 0 {
        return Ok(None);
    }

    if state
        .password_manager
        .is_password_set(repo_id, user_id)
        .await
    {
        Ok(state
            .password_manager
            .get_decrypt_key(repo_id, user_id)
            .await)
    } else {
        Err(AppError::RepoPasswdRequired)
    }
}

/// Compute the actual target directory given the base `parent_dir` and an
/// optional `relative_path` (e.g. `"myfolder/sub/"` from a folder upload).
/// When `relative_path` is empty, returns `parent_dir` unchanged.
///
/// # Errors
///
/// Returns `AppError::BadRequest` if the resulting path contains traversal
/// components that would escape the repository root, or if the path contains
/// invalid characters.
fn compute_target_dir(parent_dir: &str, relative_path: &str) -> Result<String, AppError> {
    crate::sanitize::safe_join_path(parent_dir, relative_path).map_err(|e| {
        AppError::BadRequest(format!(
            "Invalid path: {}. Please ensure the path does not contain '..' components that would escape the repository.",
            e
        ))
    })
}

// ─── Content-Range / chunked upload helpers ───────────────────────────────

/// Parse a `Content-Range` header of the form `bytes start-end/file_size`.
///
/// Example: `"bytes 0-8388607/26214400"` → `(0, 8388607, 26214400)`
fn parse_content_range(header: &str) -> Result<(u64, u64, u64), AppError> {
    let rest = header
        .strip_prefix("bytes ")
        .ok_or_else(|| AppError::BadRequest("invalid Content-Range format".into()))?;
    let (range, size_str) = rest
        .split_once('/')
        .ok_or_else(|| AppError::BadRequest("invalid Content-Range: missing file size".into()))?;
    let (start_str, end_str) = range
        .split_once('-')
        .ok_or_else(|| AppError::BadRequest("invalid Content-Range: missing range".into()))?;
    let start: u64 = start_str
        .parse()
        .map_err(|_| AppError::BadRequest("invalid Content-Range: invalid start".into()))?;
    let end: u64 = end_str
        .parse()
        .map_err(|_| AppError::BadRequest("invalid Content-Range: invalid end".into()))?;
    let file_size: u64 = size_str
        .parse()
        .map_err(|_| AppError::BadRequest("invalid Content-Range: invalid file size".into()))?;
    if end >= file_size || start > end {
        return Err(AppError::BadRequest(
            "invalid Content-Range: range out of bounds".into(),
        ));
    }
    Ok((start, end, file_size))
}

/// Handle a chunked (resumable) upload when a `Content-Range` header is
/// present.
///
/// Returns:
/// - `Ok(None)` — not a chunked upload (no Content-Range, caller should
///   handle as a regular non-chunked upload).
/// - `Ok(Some(json))` — the chunk was handled. If it was an intermediate
///   chunk the response is `{"success": true}`; if it was the final chunk
///   the response is the standard file metadata JSON array.
/// - `Err(...)` — an error occurred.
async fn try_handle_chunked(
    temp_mgr: &crate::web::temp_file::TempFileManager,
    state: &AppState,
    repo_id: &str,
    target_dir: &str,
    file_name: &str,
    file_data: &[u8],
    content_range: Option<&str>,
    modifier: &str,
    user_id: Option<i32>,
) -> Result<Option<Json<serde_json::Value>>, AppError> {
    let Some(range_header) = content_range else {
        return Ok(None); // not a chunked upload
    };

    let (start, end, file_size) = parse_content_range(range_header)?;

    // Validate the chunk size matches the declared range
    let expected_len = (end - start + 1) as usize;
    if file_data.len() != expected_len {
        return Err(AppError::BadRequest(format!(
            "Content-Range chunk size mismatch: header says {expected_len}, actual {}",
            file_data.len()
        )));
    }

    // Check total file size against server limit
    let max_bytes = state.config.server.max_upload_size_mb * 1024 * 1024;
    if max_bytes > 0 && file_size > max_bytes {
        return Err(AppError::BadRequest(format!(
            "file size {file_size} exceeds upload limit {max_bytes}"
        )));
    }

    let file_path = if target_dir == "/" {
        format!("/{file_name}")
    } else {
        format!("{}/{}", target_dir.trim_end_matches('/'), file_name)
    };

    // Ensure temp file exists
    temp_mgr
        .get_or_create(repo_id, &file_path, file_size)
        .await
        .map_err(|e| AppError::Internal(format!("temp file create failed: {e}")))?;

    // Write the chunk at the declared offset
    temp_mgr
        .write_chunk(repo_id, &file_path, start, file_data)
        .await
        .map_err(|e| AppError::Internal(format!("chunk write failed: {e}")))?;

    // Intermediate chunk — tell the client to keep sending
    if end != file_size - 1 {
        return Ok(Some(Json(json!({"success": true}))));
    }

    // ── Final chunk: assemble the complete file and commit ─────────
    let full_data = temp_mgr
        .read_complete(repo_id, &file_path)
        .await
        .ok_or_else(|| AppError::Internal("failed to read assembled temp file".into()))?;

    // Verify we got the expected number of bytes
    if full_data.len() as u64 != file_size {
        temp_mgr.abort(repo_id, &file_path).await;
        return Err(AppError::Internal(format!(
            "assembled file size {} does not match expected {file_size}",
            full_data.len()
        )));
    }

    let result = upload_and_build_response(
        state, repo_id, target_dir, file_name, &full_data, modifier, user_id, None,
    )
    .await;

    // Clean up the temp file regardless of success/failure
    temp_mgr.finish(repo_id, &file_path).await;

    result.map(|json| Some(Json(json)))
}

/// Ensure the directory at `path` exists, creating any missing intermediate
/// directories recursively. No-op if `path` is `/` or already exists.
async fn ensure_dir_recursive(
    state: &AppState,
    repo_id: &str,
    path: &str,
    email: &str,
    user_id: i32,
) -> Result<(), AppError> {
    if path == "/" {
        return Ok(());
    }

    // Quick check: if the head commit root can't be resolved, the repo is empty.
    if crate::common::util::get_head_root_id(state.db.as_ref(), repo_id)
        .await
        .is_err()
    {
        return Ok(());
    }

    let parts: Vec<&str> = path
        .trim_start_matches('/')
        .split('/')
        .filter(|p| !p.is_empty())
        .collect();

    let mut current = String::from("/");
    for part in parts {
        let next = if current == "/" {
            format!("/{part}")
        } else {
            format!("{current}/{part}")
        };

        // Check if this component already exists
        let root_id = crate::common::util::get_head_root_id(state.db.as_ref(), repo_id).await?;
        if crate::repo::fs_tree::resolve_fs_id(state.db.as_ref(), repo_id, &root_id, &next)
            .await
            .is_err()
        {
            crate::fs::service::dir::create_dir_by_path(
                state.db.as_ref(),
                &state.repos,
                email,
                user_id,
                repo_id,
                &next,
            )
            .await?;
        }
        current = next;
    }

    Ok(())
}

/// Create a file via `FileOps::create_file` and return the standard JSON
/// array response expected by the Seafile frontend:
/// `[{"id": "<fs_id>", "name": "<filename>", "size": <bytes>}]`
///
/// Dynamically determines whether the file already exists and sets the
/// `replace` flag (for `FileOps::create_file`) and `op_type` (for activity
/// logging) accordingly — upload handlers should NOT hardcode `replace`.
#[allow(clippy::too_many_arguments)]
async fn upload_and_build_response(
    state: &AppState,
    repo_id: &str,
    target_dir: &str,
    filename: &str,
    data: &[u8],
    modifier: &str,
    user_id: Option<i32>,
    enc_key: Option<(&[u8], &[u8])>,
) -> Result<serde_json::Value, AppError> {
    let fp = if target_dir == "/" {
        format!("/{}", filename)
    } else {
        format!("{}/{}", target_dir.trim_end_matches('/'), filename)
    };

    // Check if the file already exists to determine replace flag and op_type.
    let size_result =
        crate::repo::get_entry_total_size(state.db.as_ref(), &state.repos, repo_id, &fp).await;
    let file_exists = size_result.is_ok();
    let old_size = size_result.ok().unwrap_or(0);

    // Check storage quota before accepting the upload.
    if let Some(uid) = user_id {
        crate::web::quota::check_upload_quota(
            &state.repos,
            uid,
            data.len() as i64,
            state.config.storage.max_storage_bytes,
        )
        .await?;
    }

    // Ensure the target directory exists before creating the file.
    // This is needed for folder uploads where subdirectories don't exist yet.
    if let Some(uid) = user_id {
        ensure_dir_recursive(state, repo_id, target_dir, modifier, uid).await?;
    }

    let fs_id = FileOps::create_file(
        state.db.as_ref(),
        &state.repos,
        repo_id,
        target_dir,
        filename,
        data,
        modifier,
        file_exists,
        &state.block_store,
        enc_key,
    )
    .await
    .map_err(|e| AppError::Internal(format!("upload failed: {e}")))?;

    // Adjust repo size (delta = new_size - old_size).
    crate::repo::adjust_repo_size(
        state.db.as_ref(),
        &state.repos,
        repo_id,
        data.len() as i64 - old_size,
    )
    .await?;

    // Log activity if a user_id was provided.
    if let Some(uid) = user_id {
        let op_type = if file_exists { "edit" } else { "create" };
        activity_log::log_activity(
            state.db.as_ref(),
            repo_id,
            op_type,
            "file",
            &fp,
            uid,
            None,
            Some(data.len() as i64),
            Some(&fs_id),
            None,
            None,
        )
        .await;
    }

    Ok(json!([{"id": fs_id, "name": filename, "size": data.len()}]))
}

// ─── Multipart parser (for desktop-client compatibility) ──────────────────────

/// Simple multipart/form-data parser that finds a named field's value
/// and extracts the file (if any) from the multipart body.
///
/// Avoids axum's `Multipart` extractor which mysteriously fails with 400
/// on the desktop client's uploads (possibly a content-type encoding issue).
fn parse_multipart(data: &[u8], boundary: &str) -> MultipartResult {
    let boundary_str = format!("--{}", boundary);
    let btag = boundary_str.as_bytes();
    let crlf_btag = format!("\r\n--{}", boundary);
    let crlf_btag_bytes = crlf_btag.as_bytes();

    let mut result = MultipartResult {
        fields: std::collections::HashMap::new(),
        file_name: None,
        file_data: None,
    };

    let mut pos = 0;

    loop {
        // Find the next boundary (first one has no leading \r\n).
        let boundary_start = if pos == 0 && data[pos..].starts_with(btag) {
            pos
        } else if let Some(off) = data[pos..]
            .windows(crlf_btag_bytes.len())
            .position(|w| w == crlf_btag_bytes)
        {
            pos + off
        } else {
            break;
        };

        let mut boundary_end = boundary_start + btag.len();
        // Skip trailing \r\n or -- (closing)
        if boundary_end + 2 <= data.len() && &data[boundary_end..boundary_end + 2] == b"--" {
            break; // closing boundary
        }
        if boundary_end + 2 <= data.len() && &data[boundary_end..boundary_end + 2] == b"\r\n" {
            boundary_end += 2;
        }
        pos = boundary_end;

        // Find end of headers (\r\n\r\n)
        if let Some(hdr_end) = data[pos..].windows(4).position(|w| w == b"\r\n\r\n") {
            let hdr = String::from_utf8_lossy(&data[pos..pos + hdr_end]);
            pos += hdr_end + 4;

            // Body extends to the next boundary
            let body_end = if let Some(next_off) = data[pos..]
                .windows(crlf_btag_bytes.len())
                .position(|w| w == crlf_btag_bytes)
            {
                next_off
            } else {
                data.len() - pos
            };
            let body = &data[pos..pos + body_end];
            pos += body_end;

            // Trim trailing \r\n from body
            let body = body
                .strip_suffix(b"\r\n")
                .or_else(|| body.strip_suffix(b"\n"))
                .unwrap_or(body);

            let field_name = hdr
                .split(';')
                .find_map(|s| s.trim().strip_prefix("name=\"")?.split('"').next())
                .unwrap_or("");

            // Extract filename — handle both single-line and multi-line headers.
            // Qt client sends `filename="value"\r\nContent-Type: ...` so we
            // cannot use strip_suffix('"') (which would look for a trailing `"`
            // that isn't there). Instead, split on `"` to get the value.
            if let Some(fname) = hdr.split(';').find_map(|s| {
                let s = s.trim();
                s.strip_prefix("filename=\"")
                    .and_then(|rest| rest.split('"').next())
                    .map(|s| s.to_string())
            }) {
                result.file_name = Some(fname.to_string());
                result.file_data = Some(body.to_vec());
                result
                    .fields
                    .insert(field_name.to_string(), fname.to_string());
            } else {
                result.fields.insert(
                    field_name.to_string(),
                    String::from_utf8_lossy(body).to_string(),
                );
            }
        }
    }

    result
}

struct MultipartResult {
    fields: std::collections::HashMap<String, String>,
    file_name: Option<String>,
    file_data: Option<Vec<u8>>,
}

// ─── No-token web upload endpoints ────────────────────────────────────────────

/// POST /upload-aj/ — AJAX file upload (Seahub web UI, no token).
///
/// Expects multipart fields:
/// - `file` — the file bytes
/// - `repo_id` — repository ID
/// - `parent_dir` — target directory (default `/`)
/// - `relative_path` — subdirectory path for folder uploads (optional)
pub async fn upload_aj(
    user: WebUser,
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    mut multipart: Multipart,
) -> Result<Json<serde_json::Value>, AppError> {
    let mut fields: HashMap<String, String> = HashMap::new();
    let mut file_data = Vec::new();
    let mut filename = String::new();

    while let Some(field) = multipart
        .next_field()
        .await
        .map_err(|e| AppError::Internal(format!("multipart error: {e}")))?
    {
        let name = field.name().unwrap_or("").to_string();
        if name == "file" {
            filename = field.file_name().unwrap_or("unknown").to_string();
            file_data = field
                .bytes()
                .await
                .map_err(|e| AppError::Internal(format!("file read error: {e}")))?
                .to_vec();
        } else {
            fields.insert(
                name,
                field
                    .text()
                    .await
                    .map_err(|e| AppError::Internal(format!("multipart field error: {e}")))?,
            );
        }
    }

    let repo_id = fields
        .get("repo_id")
        .ok_or_else(|| AppError::BadRequest("repo_id required".into()))?;
    let parent_dir = fields.get("parent_dir").map(|s| s.as_str()).unwrap_or("/");
    let relative_path = fields
        .get("relative_path")
        .map(|s| s.as_str())
        .unwrap_or("");
    let target_dir = compute_target_dir(parent_dir, relative_path)?;

    if !file_data.is_empty() {
        // Try chunked upload path first (returns Some if Content-Range was present)
        let content_range = headers.get("content-range").and_then(|v| v.to_str().ok());
        if let Some(resp) = try_handle_chunked(
            &state.temp_file_manager,
            &state,
            repo_id,
            &target_dir,
            &filename,
            &file_data,
            content_range,
            &user.email,
            Some(user.user_id),
        )
        .await?
        {
            return Ok(resp);
        }

        // Non-chunked upload: standard path
        let resp = upload_and_build_response(
            &state,
            repo_id,
            &target_dir,
            &filename,
            &file_data,
            &user.email,
            Some(user.user_id),
            None,
        )
        .await?;
        return Ok(Json(resp));
    }

    Ok(Json(json!([{"name": filename, "uploaded": true}])))
}

/// POST /update-api/ — Update existing file (web UI, no token).
///
/// Expects multipart fields:
/// - `file` — the new file bytes
/// - `repo_id` — repository ID
/// - `p` or `path` — full path of the target file (e.g. `/dir/file.txt`)
pub async fn update_api(
    user: WebUser,
    State(state): State<Arc<AppState>>,
    mut multipart: Multipart,
) -> Result<Json<serde_json::Value>, AppError> {
    let mut repo_id = String::new();
    let mut file_path = String::new();
    let mut file_data: Vec<u8> = Vec::new();

    while let Some(field) = multipart
        .next_field()
        .await
        .map_err(|e| AppError::Internal(format!("multipart error: {e}")))?
    {
        let name = field.name().unwrap_or("").to_string();
        if name == "file" {
            file_data = field
                .bytes()
                .await
                .map_err(|e| AppError::Internal(format!("file read error: {e}")))?
                .to_vec();
        } else {
            let val = field
                .text()
                .await
                .map_err(|e| AppError::Internal(format!("multipart field error: {e}")))?;
            if name == "repo_id" {
                repo_id = val.clone();
            }
            if name == "p" || name == "path" {
                file_path = val;
            }
        }
    }

    if !file_data.is_empty() && !file_path.is_empty() {
        let parent = file_path
            .rsplit_once('/')
            .map(|(p, _)| if p.is_empty() { "/" } else { p })
            .unwrap_or("/");
        let name = file_path
            .rsplit_once('/')
            .map(|(_, n)| n)
            .unwrap_or(&file_path);

        let resp = upload_and_build_response(
            &state,
            &repo_id,
            parent,
            name,
            &file_data,
            &user.email,
            Some(user.user_id),
            None,
        )
        .await?;
        return Ok(Json(resp));
    }

    Ok(Json(json!({"success": true})))
}

/// POST /update-aj/ — AJAX file update (Seahub web UI).
///
/// Expects multipart fields:
/// - `file` — the new file bytes
/// - `repo_id` — repository ID
/// - `target_file` — full path (e.g. `/dir/file.txt`)
pub async fn update_aj(
    user: WebUser,
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    mut multipart: Multipart,
) -> Result<Json<serde_json::Value>, AppError> {
    let mut fields: HashMap<String, String> = HashMap::new();
    let mut file_data: Vec<u8> = Vec::new();

    while let Some(field) = multipart
        .next_field()
        .await
        .map_err(|e| AppError::Internal(format!("multipart error: {e}")))?
    {
        let name = field.name().unwrap_or("").to_string();
        if name == "file" {
            file_data = field
                .bytes()
                .await
                .map_err(|e| AppError::Internal(format!("file read error: {e}")))?
                .to_vec();
        } else {
            fields.insert(
                name,
                field
                    .text()
                    .await
                    .map_err(|e| AppError::Internal(format!("multipart field error: {e}")))?,
            );
        }
    }

    let repo_id = fields
        .get("repo_id")
        .ok_or_else(|| AppError::BadRequest("repo_id required".into()))?;
    let target_file = fields
        .get("target_file")
        .ok_or_else(|| AppError::BadRequest("target_file required".into()))?;

    if !file_data.is_empty() {
        let parent = target_file
            .rsplit_once('/')
            .map(|(p, _)| if p.is_empty() { "/" } else { p })
            .unwrap_or("/");
        let name = target_file
            .rsplit_once('/')
            .map(|(_, n)| n)
            .unwrap_or(target_file);

        // Try chunked upload path first
        let content_range = headers.get("content-range").and_then(|v| v.to_str().ok());
        if let Some(resp) = try_handle_chunked(
            &state.temp_file_manager,
            &state,
            repo_id,
            parent,
            name,
            &file_data,
            content_range,
            &user.email,
            Some(user.user_id),
        )
        .await?
        {
            return Ok(resp);
        }

        let resp = upload_and_build_response(
            &state,
            repo_id,
            parent,
            name,
            &file_data,
            &user.email,
            Some(user.user_id),
            None,
        )
        .await?;
        return Ok(Json(resp));
    }

    Ok(Json(json!({"success": true})))
}

// ─── Token-authenticated upload endpoints ─────────────────────────────────────

/// POST /upload-aj/{token} — Token-based AJAX file upload (Seahub web frontend).
///
/// This is the endpoint the Seahub React frontend sends uploads to after
/// obtaining an upload link from `/api2/repos/{id}/upload-link/?from=web`.
///
/// Multipart fields:
/// - `file` — the file bytes
/// - `parent_dir` — target directory
/// - `relative_path` — subdirectory path for folder uploads (optional)
pub async fn upload_aj_token(
    State(state): State<Arc<AppState>>,
    Path(token): Path<String>,
    headers: HeaderMap,
    mut multipart: Multipart,
) -> Result<Json<serde_json::Value>, AppError> {
    let info = state
        .token_manager
        .validate(&token)
        .ok_or_else(|| AppError::BadRequest("invalid or expired upload token".into()))?;

    if info.op != "upload" {
        return Err(AppError::BadRequest("token not valid for upload".into()));
    }

    let mut fields: HashMap<String, String> = HashMap::new();
    let mut file_data: Vec<u8> = Vec::new();
    let mut filename = String::new();

    while let Some(field) = multipart
        .next_field()
        .await
        .map_err(|e| AppError::Internal(format!("multipart error: {e}")))?
    {
        let name = field.name().unwrap_or("").to_string();
        if name == "file" {
            filename = field.file_name().unwrap_or("unknown").to_string();
            file_data = field
                .bytes()
                .await
                .map_err(|e| AppError::Internal(format!("file read error: {e}")))?
                .to_vec();
        } else {
            fields.insert(
                name,
                field
                    .text()
                    .await
                    .map_err(|e| AppError::Internal(format!("multipart field error: {e}")))?,
            );
        }
    }

    let parent_dir = fields
        .get("parent_dir")
        .map(|s| s.as_str())
        .unwrap_or(&info.parent_dir);
    let relative_path = fields
        .get("relative_path")
        .map(|s| s.as_str())
        .unwrap_or("");
    let target_dir = compute_target_dir(parent_dir, relative_path)?;

    if !file_data.is_empty() {
        // Try chunked upload path first
        let content_range = headers.get("content-range").and_then(|v| v.to_str().ok());
        if let Some(resp) = try_handle_chunked(
            &state.temp_file_manager,
            &state,
            &info.repo_id,
            &target_dir,
            &filename,
            &file_data,
            content_range,
            &info.username,
            None,
        )
        .await?
        {
            return Ok(resp);
        }

        let uid = activity_log::user_id_by_email(state.db.as_ref(), &info.username).await;
        let resp = upload_and_build_response(
            &state,
            &info.repo_id,
            &target_dir,
            &filename,
            &file_data,
            &info.username,
            uid,
            None,
        )
        .await?;

        // Increment upload count if this was triggered by an upload link
        if let Some(link_id) = info.upload_link_id {
            upload_link_service::increment_upload_view_cnt(state.db.clone(), link_id);
        }

        return Ok(Json(resp));
    }

    Ok(Json(json!([{"name": filename, "uploaded": true}])))
}

/// POST /upload-api/{token} — Token-authenticated file upload (desktop client).
///
/// Uses the custom `parse_multipart` helper instead of axum's `Multipart`
/// extractor because the Qt desktop client sends a quoted boundary string
/// that axum rejects with 400.
///
/// Multipart fields:
/// - `file` — the file bytes
/// - `parent_dir` — target directory (falls back to token's parent_dir)
/// - `relative_path` — subdirectory path for folder uploads (optional)
pub async fn upload_api(
    State(state): State<Arc<AppState>>,
    Path(token): Path<String>,
    req: axum::http::Request<axum::body::Body>,
) -> Result<Json<serde_json::Value>, AppError> {
    // Extract Content-Type header before consuming the request body.
    let ct = req
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_owned())
        .unwrap_or_default();

    let info = state
        .token_manager
        .validate(&token)
        .ok_or_else(|| AppError::BadRequest("invalid or expired upload token".into()))?;

    if info.op != "upload" {
        return Err(AppError::BadRequest("token not valid for upload".into()));
    }

    // Read the full body
    let bytes = axum::body::to_bytes(req.into_body(), usize::MAX)
        .await
        .map_err(|e| AppError::Internal(e.to_string()))?;

    // Extract boundary from Content-Type
    // NOTE: Qt's QHttpMultiPart sends a quoted boundary
    // (`boundary="_.Seafile._UUID"`) while the body uses the unquoted form
    // (`--_.Seafile._UUID`). Strip surrounding quotes to handle both.
    let boundary = ct
        .split("boundary=")
        .nth(1)
        .map(|s| s.trim().trim_matches('"').to_string())
        .ok_or_else(|| AppError::BadRequest("missing boundary".into()))?;

    // Parse multipart using shared helper (avoids axum Multipart extractor
    // which mysteriously fails with 400 on the desktop client's uploads).
    let parsed = parse_multipart(&bytes, &boundary);
    let parent_dir = parsed
        .fields
        .get("parent_dir")
        .cloned()
        .unwrap_or(info.parent_dir);
    let relative_path = parsed
        .fields
        .get("relative_path")
        .cloned()
        .unwrap_or_default();
    let target_dir = compute_target_dir(&parent_dir, &relative_path)?;
    let filename = parsed.file_name.unwrap_or_default();

    if let Some(data) = parsed.file_data
        && !data.is_empty()
    {
        let uid = activity_log::user_id_by_email(state.db.as_ref(), &info.username).await;
        let resp = upload_and_build_response(
            &state,
            &info.repo_id,
            &target_dir,
            &filename,
            &data,
            &info.username,
            uid,
            None,
        )
        .await?;
        return Ok(Json(resp));
    }

    Ok(Json(json!([{"name": filename, "uploaded": true}])))
}

// ─── Token-authenticated update endpoints ─────────────────────────────────────

/// POST /update-api/{token} — Token-authenticated file update / overwrite (desktop client).
///
/// Multipart fields (parsed via custom parser for Qt compat):
/// - `file` — the new file bytes
/// - `target_file` — full path of the file to overwrite (e.g. `/dir/file.txt`)
/// - `relative_path` — optional subdirectory path (prepended to target_file's parent)
/// - `parent_dir` — fallback base directory when target_file is absent
pub async fn update_api_handler(
    State(state): State<Arc<AppState>>,
    Path(token): Path<String>,
    req: axum::http::Request<axum::body::Body>,
) -> Result<Json<serde_json::Value>, AppError> {
    let ct = req
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_owned())
        .unwrap_or_default();

    let info = state
        .token_manager
        .validate(&token)
        .ok_or_else(|| AppError::BadRequest("invalid or expired update token".into()))?;

    if info.op != "update" {
        return Err(AppError::BadRequest("token not valid for update".into()));
    }

    let bytes = axum::body::to_bytes(req.into_body(), usize::MAX)
        .await
        .map_err(|e| AppError::Internal(e.to_string()))?;

    let parsed = ct
        .split("boundary=")
        .nth(1)
        .map(|s| parse_multipart(&bytes, s.trim().trim_matches('"')))
        .unwrap_or(MultipartResult {
            fields: std::collections::HashMap::new(),
            file_name: None,
            file_data: None,
        });

    let data = parsed.file_data.unwrap_or_default();
    let target_file = parsed
        .fields
        .get("target_file")
        .cloned()
        .unwrap_or_default();
    let relative_path = parsed
        .fields
        .get("relative_path")
        .cloned()
        .unwrap_or_default();

    if !data.is_empty() {
        let uid = activity_log::user_id_by_email(state.db.as_ref(), &info.username).await;
        if !target_file.is_empty() {
            // Derive target from target_file + optional relative_path
            let (raw_parent, raw_name) =
                target_file.rsplit_once('/').unwrap_or(("/", &target_file));
            let parent = if raw_parent.is_empty() {
                "/"
            } else {
                raw_parent
            };
            let target_dir = compute_target_dir(parent, &relative_path)?;
            let name = raw_name.to_string();

            // Get old file size for incremental size adjustment.
            let fp = if target_dir == "/" {
                format!("/{}", name)
            } else {
                format!("{}/{}", target_dir.trim_end_matches('/'), name)
            };
            let size_result = crate::repo::get_entry_total_size(
                state.db.as_ref(),
                &state.repos,
                &info.repo_id,
                &fp,
            )
            .await;
            let file_exists = size_result.is_ok();
            let old_size = size_result.ok().unwrap_or(0);

            let fs_id = FileOps::create_file(
                state.db.as_ref(),
                &state.repos,
                &info.repo_id,
                &target_dir,
                &name,
                &data,
                &info.username,
                file_exists,
                &state.block_store,
                None,
            )
            .await
            .map_err(|e| AppError::Internal(format!("update failed: {e}")))?;

            // Adjust repo size.
            crate::repo::adjust_repo_size(
                state.db.as_ref(),
                &state.repos,
                &info.repo_id,
                data.len() as i64 - old_size,
            )
            .await?;

            // Log activity
            if let Some(uid) = uid {
                let op_type = if file_exists { "edit" } else { "create" };
                activity_log::log_activity(
                    state.db.as_ref(),
                    &info.repo_id,
                    op_type,
                    "file",
                    &fp,
                    uid,
                    None,
                    Some(data.len() as i64),
                    Some(&fs_id),
                    None,
                    None,
                )
                .await;
            }

            return Ok(Json(
                json!([{"id": fs_id, "name": name, "size": data.len()}]),
            ));
        }

        // Fallback: parent_dir + relative_path + filename
        let filename = parsed.file_name.unwrap_or_default();
        let parent_dir = parsed
            .fields
            .get("parent_dir")
            .cloned()
            .unwrap_or(info.parent_dir);
        let target_dir = compute_target_dir(&parent_dir, &relative_path)?;

        if !filename.is_empty() {
            let resp = upload_and_build_response(
                &state,
                &info.repo_id,
                &target_dir,
                &filename,
                &data,
                &info.username,
                uid,
                None,
            )
            .await?;
            return Ok(Json(resp));
        }
    }

    Ok(Json(json!({"success": true})))
}

/// POST /update-aj/{token} — Token-based AJAX file update (Seahub web frontend).
///
/// Multipart fields:
/// - `file` — the new file bytes
/// - `target_file` — full path (e.g. `/dir/file.txt`)
/// - `relative_path` — optional subdirectory path
pub async fn update_aj_token(
    State(state): State<Arc<AppState>>,
    Path(token): Path<String>,
    mut multipart: Multipart,
) -> Result<Json<serde_json::Value>, AppError> {
    let info = state
        .token_manager
        .validate(&token)
        .ok_or_else(|| AppError::BadRequest("invalid or expired update token".into()))?;

    if info.op != "update" {
        return Err(AppError::BadRequest("token not valid for update".into()));
    }

    let mut fields: HashMap<String, String> = HashMap::new();
    let mut file_data: Vec<u8> = Vec::new();

    while let Some(field) = multipart
        .next_field()
        .await
        .map_err(|e| AppError::Internal(format!("multipart error: {e}")))?
    {
        let name = field.name().unwrap_or("").to_string();
        if name == "file" {
            file_data = field
                .bytes()
                .await
                .map_err(|e| AppError::Internal(format!("file read error: {e}")))?
                .to_vec();
        } else {
            fields.insert(
                name,
                field
                    .text()
                    .await
                    .map_err(|e| AppError::Internal(format!("multipart field error: {e}")))?,
            );
        }
    }

    if file_data.is_empty() {
        return Ok(Json(json!({"success": true})));
    }

    let uid = activity_log::user_id_by_email(state.db.as_ref(), &info.username).await;
    let target_file = fields.get("target_file").cloned().unwrap_or_default();
    let relative_path = fields.get("relative_path").cloned().unwrap_or_default();

    if !target_file.is_empty() {
        let slash_pos = target_file.rfind('/').unwrap_or(0);
        let raw_parent = if slash_pos == 0 {
            "/"
        } else {
            &target_file[..slash_pos]
        };
        let name = target_file[slash_pos + 1..].to_string();
        let target_dir = compute_target_dir(raw_parent, &relative_path)?;

        // Get old file size for incremental size adjustment.
        let fp = if target_dir == "/" {
            format!("/{}", name)
        } else {
            format!("{}/{}", target_dir.trim_end_matches('/'), name)
        };
        let size_result =
            crate::repo::get_entry_total_size(state.db.as_ref(), &state.repos, &info.repo_id, &fp)
                .await;
        let file_exists = size_result.is_ok();
        let old_size = size_result.ok().unwrap_or(0);

        let fs_id = FileOps::create_file(
            state.db.as_ref(),
            &state.repos,
            &info.repo_id,
            &target_dir,
            &name,
            &file_data,
            &info.username,
            file_exists,
            &state.block_store,
            None,
        )
        .await
        .map_err(|e| AppError::Internal(format!("update failed: {e}")))?;

        // Adjust repo size.
        crate::repo::adjust_repo_size(
            state.db.as_ref(),
            &state.repos,
            &info.repo_id,
            file_data.len() as i64 - old_size,
        )
        .await?;

        // Log activity
        if let Some(uid) = uid {
            let op_type = if file_exists { "edit" } else { "create" };
            activity_log::log_activity(
                state.db.as_ref(),
                &info.repo_id,
                op_type,
                "file",
                &fp,
                uid,
                None,
                Some(file_data.len() as i64),
                Some(&fs_id),
                None,
                None,
            )
            .await;
        }

        return Ok(Json(
            json!([{"id": fs_id, "name": name, "size": file_data.len()}]),
        ));
    }

    Ok(Json(json!({"success": true})))
}

/// POST /upload-blks-api/{token} — Token-based block upload and commit.
///
/// Two modes:
///
/// **Block upload mode** (no `commitonly` field):
/// Accepts multipart with `file` parts (one per block, filename = block ID).
/// Validates SHA1 matches the block ID, stores each block.
///
/// **Commit mode** (with `commitonly` field):
/// Multipart fields:
/// - `commitonly` — must be present (any value)
/// - `parent_dir` — target directory
/// - `file_name` — name of the assembled file
/// - `blockids` — JSON array of block IDs: `["id1","id2"]`
/// - `file_size` — total file size in bytes
/// - `replace` — "1" to overwrite (optional)
/// - `last_modify` — ISO timestamp (optional)
///
/// Response: `{"id": "<file_fs_id>"}` on success.
pub async fn upload_blks_api(
    State(state): State<Arc<AppState>>,
    Path(token): Path<String>,
    mut multipart: Multipart,
) -> Result<Json<serde_json::Value>, AppError> {
    let info = state
        .token_manager
        .validate(&token)
        .ok_or_else(|| AppError::BadRequest("invalid or expired upload token".into()))?;

    if info.op != "upload-blks" && info.op != "update-blks" {
        return Err(AppError::BadRequest(
            "token not valid for block upload".into(),
        ));
    }

    let uid = activity_log::user_id_by_email(state.db.as_ref(), &info.username).await;
    let mut fields: HashMap<String, String> = HashMap::new();
    let mut blocks: Vec<(String, Vec<u8>)> = Vec::new();

    while let Some(field) = multipart
        .next_field()
        .await
        .map_err(|e| AppError::Internal(format!("multipart error: {e}")))?
    {
        let name = field.name().unwrap_or("").to_string();
        if name == "file" {
            let block_id = field.file_name().unwrap_or("").to_string();
            let data = field
                .bytes()
                .await
                .map_err(|e| AppError::Internal(format!("block read error: {e}")))?
                .to_vec();
            if !block_id.is_empty() && !data.is_empty() {
                blocks.push((block_id, data));
            }
        } else {
            fields.insert(
                name,
                field
                    .text()
                    .await
                    .map_err(|e| AppError::Internal(format!("multipart field error: {e}")))?,
            );
        }
    }

    // Check if this is a commit request
    if fields.contains_key("commitonly") {
        let parent_dir = fields
            .get("parent_dir")
            .map(|s| s.as_str())
            .unwrap_or(&info.parent_dir);
        let file_name = fields
            .get("file_name")
            .ok_or_else(|| AppError::BadRequest("file_name required for commit".into()))?;
        let blockids_str = fields
            .get("blockids")
            .ok_or_else(|| AppError::BadRequest("blockids required for commit".into()))?;
        let file_size: i64 = fields
            .get("file_size")
            .and_then(|s| s.parse().ok())
            .ok_or_else(|| AppError::BadRequest("file_size required for commit".into()))?;
        let replace = fields.get("replace").map(|s| s.as_str()) == Some("1");

        // Parse blockids JSON array
        let block_ids: Vec<String> = serde_json::from_str(blockids_str)
            .map_err(|_| AppError::BadRequest("invalid blockids JSON array".into()))?;

        if block_ids.is_empty() {
            return Err(AppError::BadRequest("blockids cannot be empty".into()));
        }

        // Verify all blocks exist in block store
        for bid in &block_ids {
            if !state.block_store.has_block(bid).await {
                return Err(AppError::BadRequest(format!("block not found: {bid}")));
            }
        }

        // Create FsFileData from block IDs
        let file_fs_data = crate::serialization::fs_json::FsFileData {
            block_ids: block_ids.clone(),
            size: file_size,
            obj_type: 1,
            version: 1,
        };
        let file_fs_id = file_fs_data
            .compute_and_store(state.db.as_ref(), &info.repo_id)
            .await?;

        // Update directory tree and create commit
        let relative_path = fields
            .get("relative_path")
            .map(|s| s.as_str())
            .unwrap_or("");
        let target_dir = compute_target_dir(parent_dir, relative_path)?;
        let now = chrono::Utc::now().timestamp();

        // Get old file size for incremental size adjustment
        // and determine if the file already exists (for activity logging op_type).
        let fp = if target_dir == "/" {
            format!("/{}", file_name)
        } else {
            format!("{}/{}", target_dir.trim_end_matches('/'), file_name)
        };
        let size_result =
            crate::repo::get_entry_total_size(state.db.as_ref(), &state.repos, &info.repo_id, &fp)
                .await;
        let file_exists = size_result.is_ok();
        let old_size = size_result.ok().unwrap_or(0);

        // Resolve parent directory and capture ancestor chain for the
        // subsequent walk_up_ancestors (avoids O(d²) re-resolution).
        let (parent_fs_id, ancestor_chain) = crate::repo::file_ops::FileOps::resolve_fs_id_chain(
            state.db.as_ref(),
            &info.repo_id,
            &target_dir,
        )
        .await
        .map_err(|e| AppError::Internal(format!("resolve parent dir failed: {e}")))?;

        // Add file entry to parent directory
        let entry_name = file_name.clone();
        let modifier_name = info.username.clone();
        crate::repo::file_ops::FileOps::update_dir_tree_and_commit(
            state.db.as_ref(),
            &state.repos,
            &info.repo_id,
            &target_dir,
            &parent_fs_id,
            &modifier_name,
            &format!("Added {file_name}"),
            &ancestor_chain,
            |dirents| {
                if replace {
                    dirents.retain(|d| d.name != entry_name);
                }
                // Handle name collision
                if dirents.iter().any(|d| d.name == entry_name) {
                    let unique_name =
                        crate::common::util::generate_unique_filename(dirents, &entry_name);
                    dirents.push(crate::serialization::fs_json::DirEntryData {
                        id: file_fs_id.clone(),
                        mode: crate::serialization::S_IFREG,
                        modifier: modifier_name.clone(),
                        mtime: now,
                        name: unique_name,
                        size: file_size,
                    });
                } else {
                    dirents.push(crate::serialization::fs_json::DirEntryData {
                        id: file_fs_id.clone(),
                        mode: crate::serialization::S_IFREG,
                        modifier: modifier_name.clone(),
                        mtime: now,
                        name: entry_name.clone(),
                        size: file_size,
                    });
                }
                Ok(())
            },
        )
        .await
        .map_err(|e| AppError::Internal(format!("commit blocks failed: {e}")))?;

        // Adjust repo size
        crate::repo::adjust_repo_size(
            state.db.as_ref(),
            &state.repos,
            &info.repo_id,
            file_size - old_size,
        )
        .await?;

        // Log activity — op_type based on actual file existence, not client's replace flag
        let op_type = if file_exists { "edit" } else { "create" };
        if let Some(uid) = uid {
            activity_log::log_activity(
                state.db.as_ref(),
                &info.repo_id,
                op_type,
                "file",
                &fp,
                uid,
                None,
                Some(file_size),
                Some(&file_fs_id),
                None,
                None,
            )
            .await;
        }

        return Ok(Json(json!({"id": file_fs_id})));
    }

    // Block upload mode: verify SHA1 and store each block
    for (block_id, data) in &blocks {
        // Compute SHA1 of the data and verify it matches the block_id
        let computed_id = {
            use sha1::{Digest, Sha1};
            let mut hasher = Sha1::new();
            hasher.update(data);
            hex::encode(hasher.finalize())
        };

        if computed_id != *block_id {
            return Err(AppError::BadRequest(format!(
                "block ID mismatch: expected {block_id}, computed {computed_id}"
            )));
        }

        state
            .block_store
            .write_block(data)
            .await
            .map_err(|e| AppError::Internal(format!("failed to write block {block_id}: {e}")))?;
    }

    Ok(Json(json!({"success": true})))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// parse_multipart must correctly extract field names even when extra
    /// headers (e.g. Content-Length) follow the Content-Disposition line.
    /// OkHttp (used by the Android client) adds Content-Length to each part:
    ///
    ///   Content-Disposition: form-data; name="parent_dir"\r\n
    ///   Content-Length: 1\r\n
    ///
    /// The earlier field-name extraction used strip_suffix('"') which assumed
    /// the Content-Disposition line is the only header and always ends with a
    /// closing quote. When extra headers follow, strip_suffix('"') returned
    /// None and the field name was silently lost, causing all uploads to go
    /// to root regardless of parent_dir or relative_path.
    #[test]
    fn test_parse_multipart_with_content_length() {
        let boundary = "testboundary";
        let body = format!(
            "\
            --{boundary}\r\n\
            Content-Disposition: form-data; name=\"parent_dir\"\r\n\
            Content-Length: 1\r\n\
            \r\n\
            /\r\n\
            --{boundary}\r\n\
            Content-Disposition: form-data; name=\"relative_path\"\r\n\
            Content-Length: 16\r\n\
            \r\n\
            My Photos/Camera/\r\n\
            --{boundary}--\r\n"
        );

        let result = parse_multipart(body.as_bytes(), boundary);
        assert_eq!(
            result.fields.get("parent_dir").map(|s| s.as_str()),
            Some("/")
        );
        assert_eq!(
            result.fields.get("relative_path").map(|s| s.as_str()),
            Some("My Photos/Camera/")
        );
    }

    /// parse_multipart must still work without extra headers (simple case).
    #[test]
    fn test_parse_multipart_simple() {
        let boundary = "simple";
        let body = format!(
            "\
            --{boundary}\r\n\
            Content-Disposition: form-data; name=\"field1\"\r\n\
            \r\n\
            value1\r\n\
            --{boundary}\r\n\
            Content-Disposition: form-data; name=\"field2\"\r\n\
            \r\n\
            value2\r\n\
            --{boundary}--\r\n"
        );

        let result = parse_multipart(body.as_bytes(), boundary);
        assert_eq!(
            result.fields.get("field1").map(|s| s.as_str()),
            Some("value1")
        );
        assert_eq!(
            result.fields.get("field2").map(|s| s.as_str()),
            Some("value2")
        );
    }

    /// parse_multipart must extract file parts with extra headers (Content-Type).
    #[test]
    fn test_parse_multipart_with_file_and_content_type() {
        let boundary = "filebound";
        let body = format!(
            "\
            --{boundary}\r\n\
            Content-Disposition: form-data; name=\"file\"; filename=\"photo.jpg\"\r\n\
            Content-Type: image/jpeg\r\n\
            Content-Length: 10\r\n\
            \r\n\
            filedata\r\n\
            --{boundary}--\r\n"
        );

        let result = parse_multipart(body.as_bytes(), boundary);
        assert_eq!(result.file_name.as_deref(), Some("photo.jpg"));
        assert_eq!(result.file_data.as_deref(), Some(&b"filedata"[..]));
    }
}
