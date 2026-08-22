use axum::{
    Json,
    extract::{Multipart, Path, State},
    http::HeaderMap,
};
use serde_json::json;
use std::collections::HashMap;
use std::sync::Arc;

use crate::AppState;
use crate::handler::ok_json;
use crate::service::sharing::link as upload_link_service;
use crate::ui::auth_extractor::WebUser;
use base::error::AppError;

// ─── Helpers ─────────────────────────────────────────────────────────────────

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
    base::sanitize::safe_join_path(parent_dir, relative_path).map_err(|e| {
        AppError::BadRequest(format!(
            "Invalid path: {}. Please ensure the path does not contain '..' components that would escape the repository.",
            e
        ))
    })
}

/// Compute the target directory for a token-authenticated upload, scoping
/// upload-link tokens to the directory the link was created for. Upload links
/// must not be able to write outside their directory via a client-supplied
/// `parent_dir` or `relative_path`.
fn compute_scoped_target_dir(
    info: &crate::service::auth::access_token::AccessToken,
    client_parent_dir: &str,
    relative_path: &str,
) -> Result<String, AppError> {
    if info.upload_link_id.is_some() {
        // Upload links are scoped to their directory: ignore the client's
        // `parent_dir` and resolve relative to the link's own path, then ensure
        // the result stays inside it. (`safe_join_path` only clamps traversal to
        // the repo root, not to the link's directory.)
        let target = base::sanitize::safe_join_path(&info.parent_dir, relative_path)
            .map_err(|e| AppError::BadRequest(format!("Invalid path: {e}")))?;
        let root = base::sanitize::safe_normalize_path(&info.parent_dir)
            .unwrap_or_else(|_| "/".to_string());
        if root != "/"
            && target != root
            && !target.starts_with(&format!("{}/", root.trim_end_matches('/')))
        {
            return Err(AppError::BadRequest(
                "path outside upload link directory".into(),
            ));
        }
        Ok(target)
    } else {
        compute_target_dir(client_parent_dir, relative_path)
    }
}

// ─── Content-Range / chunked upload helpers ───────────────────────────────

/// Parse a `Content-Range` header of the form `bytes start-end/file_size`.
///
/// Example: `"bytes 0-8388607/26214400"` → `(0, 8388607, 26214400)`
/// Validate an upload-link commit `file_name`, aligned with seafile's
/// `should_ignore_file` semantics (reject empty, path separators, NUL, "." /
/// "..", and over-long names) rather than a strict character blacklist.
fn is_valid_upload_filename(name: &str) -> bool {
    !name.is_empty()
        && name != "."
        && name != ".."
        && !name.contains('/')
        && !name.contains('\0')
        && name.len() <= 255
}

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
    temp_mgr: &crate::handler::web::temp_file::TempFileManager,
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

    // Pre-check storage quota once (on the first chunk) against the declared
    // total size so over-quota uploads fail before consuming the whole file.
    // The result only depends on `file_size`, so re-checking every chunk is
    // wasted work; the final assembly path re-checks quota as a backstop.
    if start == 0
        && let Some(uid) = user_id
    {
        crate::service::fs::quota::check_upload_quota(
            &state.repos,
            uid,
            file_size as i64,
            state.config.storage.max_storage_bytes,
        )
        .await?;
    }

    let file_path = base::sanitize::safe_join_path(target_dir, file_name)
        .map_err(|e| AppError::BadRequest(format!("invalid path: {e}")))?;

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
        return Ok(Some(ok_json()));
    }

    // ── Final chunk: stream the assembled file into blocks and commit ──
    // Stream the temp file through the CDC chunker so the whole file never
    // has to be buffered in memory; `write_stream_blocks` writes blocks to
    // the content-addressed store and `upload_file_committed_stream` commits
    // the resulting block_ids into the repo.
    let Some(stream) = temp_mgr.read_stream(repo_id, &file_path).await else {
        temp_mgr.abort(repo_id, &file_path).await;
        return Err(AppError::Internal(
            "failed to open assembled temp file".into(),
        ));
    };

    let (block_ids, total_size) = crate::fs::core::FileOps::write_stream_blocks(
        &state.block_store,
        file_size as usize,
        stream,
        None,
    )
    .await?;

    // Verify we got the expected number of bytes (the streamed total must
    // match the declared file size).
    if total_size as u64 != file_size {
        temp_mgr.abort(repo_id, &file_path).await;
        return Err(AppError::Internal(format!(
            "assembled file size {total_size} does not match expected {file_size}"
        )));
    }

    let fs_id = state
        .file_service()
        .upload_file_committed_stream(
            repo_id, target_dir, file_name, block_ids, total_size, modifier, user_id, true, None,
        )
        .await?;

    // Clean up the temp file regardless of success/failure
    temp_mgr.finish(repo_id, &file_path).await;

    Ok(Some(Json(json!([
        { "id": fs_id, "name": file_name, "size": total_size }
    ]))))
}

// ─── Multipart parser (for desktop-client compatibility) ──────────────────────

/// Simple multipart/form-data parser that finds a named field's value
/// and extracts the file (if any) from the multipart body.
///
/// Avoids axum's `Multipart` extractor which mysteriously fails with 400
/// on the desktop client's uploads (possibly a content-type encoding issue).
#[cfg(test)]
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

#[cfg(test)]
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
    body: axum::body::Body,
) -> Result<Json<serde_json::Value>, AppError> {
    let boundary = extract_multipart_boundary(&headers)?;
    // Multer's Multipart consumes the raw body stream, letting the file part
    // be read incrementally via `field.chunk()` (axum's `Multipart` extractor
    // only offers whole-field `bytes()`).
    let mut multipart = multer::Multipart::new(body.into_data_stream(), boundary);

    let mut fields: HashMap<String, String> = HashMap::new();
    let mut filename = String::new();

    // Resumable (Content-Range) uploads send the whole part at once — the
    // resumable path needs the bytes in hand, so it keeps the buffered read.
    let is_chunked = headers.get("content-range").is_some();
    let mut chunked_file_data: Option<Vec<u8>> = None;
    // Streaming (non-chunked) upload: CDC the file straight into blocks.
    let mut block_ids: Vec<String> = Vec::new();
    let mut total_size: i64 = 0;

    while let Some(mut field) = multipart
        .next_field()
        .await
        .map_err(|e| AppError::Internal(format!("multipart error: {e}")))?
    {
        let name = field.name().unwrap_or("").to_string();
        if name == "file" {
            filename = field.file_name().unwrap_or("unknown").to_string();
            if is_chunked {
                chunked_file_data = Some(
                    field
                        .bytes()
                        .await
                        .map_err(|e| AppError::Internal(format!("file read error: {e}")))?
                        .to_vec(),
                );
            } else {
                // Stream: feed underlying bytes straight into the CDC chunker
                // and write each chunk to the block store, so the file never
                // needs to be fully buffered in memory.
                let mut chunker = infra::storage::cdc::Chunker::new(0);
                let store = state.block_store.clone();
                while let Some(c) = field
                    .chunk()
                    .await
                    .map_err(|e| AppError::Internal(format!("file read error: {e}")))?
                {
                    for block in chunker.feed(&c) {
                        total_size += block.len() as i64;
                        let bid = store
                            .write_block(&block)
                            .await
                            .map_err(|e| AppError::Internal(format!("block write failed: {e}")))?;
                        block_ids.push(bid);
                    }
                }
                let last = chunker.finish();
                if !last.is_empty() {
                    total_size += last.len() as i64;
                    let bid = store
                        .write_block(&last)
                        .await
                        .map_err(|e| AppError::Internal(format!("block write failed: {e}")))?;
                    block_ids.push(bid);
                }
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

    let repo_id = fields
        .get("repo_id")
        .ok_or_else(|| AppError::BadRequest("repo_id required".into()))?;
    let parent_dir = fields.get("parent_dir").map(|s| s.as_str()).unwrap_or("/");
    let relative_path = fields
        .get("relative_path")
        .map(|s| s.as_str())
        .unwrap_or("");
    let target_dir = compute_target_dir(parent_dir, relative_path)?;

    // Authorization: this endpoint trusts the client-supplied repo_id, so the
    // caller must actually be a member with write access to the repo.
    crate::domain::permission::check_repo_write_permission(
        state.repos.member.as_ref(),
        repo_id,
        user.user_id,
    )
    .await?;

    if is_chunked {
        let file_data = chunked_file_data.unwrap_or_default();
        if !file_data.is_empty() {
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
        }
        return Ok(Json(json!([{"name": filename, "uploaded": true}])));
    }

    if !block_ids.is_empty() {
        let fs_id = state
            .file_service()
            .upload_file_committed_stream(
                repo_id,
                &target_dir,
                &filename,
                block_ids,
                total_size,
                &user.email,
                Some(user.user_id),
                true,
                None,
            )
            .await?;
        return Ok(Json(
            json!([{"id": fs_id, "name": filename, "size": total_size}]),
        ));
    }

    Ok(Json(json!([{"name": filename, "uploaded": true}])))
}

/// Extract the multipart `boundary` from the Content-Type header.
fn extract_multipart_boundary(headers: &HeaderMap) -> Result<String, AppError> {
    let ct = headers
        .get(axum::http::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .ok_or_else(|| AppError::BadRequest("missing content-type".into()))?;
    ct.split("boundary=")
        .nth(1)
        .map(|s| s.trim().trim_matches('"').to_string())
        .ok_or_else(|| AppError::BadRequest("missing multipart boundary".into()))
}

/// Stream a multipart file field straight into content-defined blocks in the
/// block store, returning the block ids and total size, so the file is never
/// fully buffered in memory. `Chunker::new(0)` uses the default (sub-2GB)
/// chunk sizing; pass a known size for larger files.
pub(crate) async fn stream_file_into_blocks(
    store: infra::storage::DynBlockStorage,
    field: &mut multer::Field<'_>,
) -> Result<(Vec<String>, i64), AppError> {
    let mut chunker = infra::storage::cdc::Chunker::new(0);
    let mut block_ids = Vec::new();
    let mut total_size = 0i64;
    while let Some(c) = field
        .chunk()
        .await
        .map_err(|e| AppError::Internal(format!("file read error: {e}")))?
    {
        for block in chunker.feed(&c) {
            total_size += block.len() as i64;
            let bid = store
                .write_block(&block)
                .await
                .map_err(|e| AppError::Internal(format!("block write failed: {e}")))?;
            block_ids.push(bid);
        }
    }
    let last = chunker.finish();
    if !last.is_empty() {
        total_size += last.len() as i64;
        let bid = store
            .write_block(&last)
            .await
            .map_err(|e| AppError::Internal(format!("block write failed: {e}")))?;
        block_ids.push(bid);
    }
    Ok((block_ids, total_size))
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
    headers: HeaderMap,
    body: axum::body::Body,
) -> Result<Json<serde_json::Value>, AppError> {
    let boundary = extract_multipart_boundary(&headers)?;
    let mut multipart = multer::Multipart::new(body.into_data_stream(), boundary);

    let mut repo_id = String::new();
    let mut file_path = String::new();
    let mut block_ids: Vec<String> = Vec::new();
    let mut total_size: i64 = 0;

    while let Some(mut field) = multipart
        .next_field()
        .await
        .map_err(|e| AppError::Internal(format!("multipart error: {e}")))?
    {
        let name = field.name().unwrap_or("").to_string();
        if name == "file" {
            let (bids, size) =
                stream_file_into_blocks(state.block_store.clone(), &mut field).await?;
            block_ids = bids;
            total_size = size;
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

    if !block_ids.is_empty() && !file_path.is_empty() {
        // Authorization: repo_id is client-supplied, so require write access.
        crate::domain::permission::check_repo_write_permission(
            state.repos.member.as_ref(),
            &repo_id,
            user.user_id,
        )
        .await?;

        let parent = file_path
            .rsplit_once('/')
            .map(|(p, _)| if p.is_empty() { "/" } else { p })
            .unwrap_or("/");
        let name = file_path
            .rsplit_once('/')
            .map(|(_, n)| n)
            .unwrap_or(&file_path);

        let fs_id = state
            .file_service()
            .upload_file_committed_stream(
                &repo_id,
                parent,
                name,
                block_ids,
                total_size,
                &user.email,
                Some(user.user_id),
                true,
                None,
            )
            .await?;
        return Ok(Json(
            json!([{"id": fs_id, "name": name, "size": total_size}]),
        ));
    }

    Ok(ok_json())
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
    body: axum::body::Body,
) -> Result<Json<serde_json::Value>, AppError> {
    let boundary = extract_multipart_boundary(&headers)?;
    let mut multipart = multer::Multipart::new(body.into_data_stream(), boundary);

    let mut fields: HashMap<String, String> = HashMap::new();
    let is_chunked = headers.get("content-range").is_some();
    let mut chunked_file_data: Option<Vec<u8>> = None;
    let mut block_ids: Vec<String> = Vec::new();
    let mut total_size: i64 = 0;

    while let Some(mut field) = multipart
        .next_field()
        .await
        .map_err(|e| AppError::Internal(format!("multipart error: {e}")))?
    {
        let name = field.name().unwrap_or("").to_string();
        if name == "file" {
            if is_chunked {
                chunked_file_data = Some(
                    field
                        .bytes()
                        .await
                        .map_err(|e| AppError::Internal(format!("file read error: {e}")))?
                        .to_vec(),
                );
            } else {
                let (bids, size) =
                    stream_file_into_blocks(state.block_store.clone(), &mut field).await?;
                block_ids = bids;
                total_size = size;
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

    let repo_id = fields
        .get("repo_id")
        .ok_or_else(|| AppError::BadRequest("repo_id required".into()))?;
    let target_file = fields
        .get("target_file")
        .ok_or_else(|| AppError::BadRequest("target_file required".into()))?;

    // Authorization: the repo_id is client-supplied, so require write access.
    crate::domain::permission::check_repo_write_permission(
        state.repos.member.as_ref(),
        repo_id,
        user.user_id,
    )
    .await?;

    let parent = target_file
        .rsplit_once('/')
        .map(|(p, _)| if p.is_empty() { "/" } else { p })
        .unwrap_or("/");
    let name = target_file
        .rsplit_once('/')
        .map(|(_, n)| n)
        .unwrap_or(target_file);

    if is_chunked {
        let file_data = chunked_file_data.unwrap_or_default();
        if !file_data.is_empty() {
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
        }
        return Ok(ok_json());
    }

    if !block_ids.is_empty() {
        let fs_id = state
            .file_service()
            .upload_file_committed_stream(
                repo_id,
                parent,
                name,
                block_ids,
                total_size,
                &user.email,
                Some(user.user_id),
                true,
                None,
            )
            .await?;
        return Ok(Json(
            json!([{"id": fs_id, "name": name, "size": total_size}]),
        ));
    }

    Ok(ok_json())
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
    body: axum::body::Body,
) -> Result<Json<serde_json::Value>, AppError> {
    let info = state
        .token_manager
        .validate(&token)
        .ok_or_else(|| AppError::BadRequest("invalid or expired upload token".into()))?;

    if info.op != "upload" {
        return Err(AppError::BadRequest("token not valid for upload".into()));
    }

    // Re-check write permission: the token was issued against the caller's
    // membership, but that may have been revoked since (matches download).
    crate::domain::permission::check_repo_write_permission(
        state.repos.member.as_ref(),
        &info.repo_id,
        info.user_id,
    )
    .await?;

    let boundary = extract_multipart_boundary(&headers)?;
    let mut multipart = multer::Multipart::new(body.into_data_stream(), boundary);

    let mut fields: HashMap<String, String> = HashMap::new();
    let mut filename = String::new();
    let is_chunked = headers.get("content-range").is_some();
    let mut chunked_file_data: Option<Vec<u8>> = None;
    let mut block_ids: Vec<String> = Vec::new();
    let mut total_size: i64 = 0;

    while let Some(mut field) = multipart
        .next_field()
        .await
        .map_err(|e| AppError::Internal(format!("multipart error: {e}")))?
    {
        let name = field.name().unwrap_or("").to_string();
        if name == "file" {
            filename = field.file_name().unwrap_or("unknown").to_string();
            if is_chunked {
                chunked_file_data = Some(
                    field
                        .bytes()
                        .await
                        .map_err(|e| AppError::Internal(format!("file read error: {e}")))?
                        .to_vec(),
                );
            } else {
                let (bids, size) =
                    stream_file_into_blocks(state.block_store.clone(), &mut field).await?;
                block_ids = bids;
                total_size = size;
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

    let parent_dir = fields
        .get("parent_dir")
        .map(|s| s.as_str())
        .unwrap_or(&info.parent_dir);
    let relative_path = fields
        .get("relative_path")
        .map(|s| s.as_str())
        .unwrap_or("");
    let target_dir = compute_scoped_target_dir(&info, parent_dir, relative_path)?;

    if is_chunked {
        let file_data = chunked_file_data.unwrap_or_default();
        if !file_data.is_empty() {
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
        }
        return Ok(Json(json!([{"name": filename, "uploaded": true}])));
    }

    if !block_ids.is_empty() {
        let uid = Some(info.user_id);
        let fs_id = state
            .file_service()
            .upload_file_committed_stream(
                &info.repo_id,
                &target_dir,
                &filename,
                block_ids,
                total_size,
                &info.username,
                uid,
                true,
                None,
            )
            .await?;

        // Increment upload count if this was triggered by an upload link
        if let Some(link_id) = info.upload_link_id {
            upload_link_service::increment_upload_view_cnt(state.repos.clone(), link_id);
        }

        return Ok(Json(
            json!([{"id": fs_id, "name": filename, "size": total_size}]),
        ));
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

    // Re-check write permission: the token was issued against the caller's
    // membership, but that may have been revoked since (matches download).
    crate::domain::permission::check_repo_write_permission(
        state.repos.member.as_ref(),
        &info.repo_id,
        info.user_id,
    )
    .await?;

    // Extract boundary from Content-Type. NOTE: Qt's QHttpMultiPart sends a
    // quoted boundary (`boundary="_.Seafile._UUID"`) while the body uses the
    // unquoted form (`--_.Seafile._UUID`) — strip surrounding quotes to handle
    // both. The body is consumed as a stream so the file is never buffered.
    let boundary = ct
        .split("boundary=")
        .nth(1)
        .map(|s| s.trim().trim_matches('"').to_string())
        .ok_or_else(|| AppError::BadRequest("missing boundary".into()))?;
    let mut multipart = multer::Multipart::new(req.into_body().into_data_stream(), boundary);

    let mut fields: HashMap<String, String> = HashMap::new();
    let mut filename = String::new();
    let mut block_ids: Vec<String> = Vec::new();
    let mut total_size: i64 = 0;

    while let Some(mut field) = multipart
        .next_field()
        .await
        .map_err(|e| AppError::Internal(format!("multipart error: {e}")))?
    {
        let name = field.name().unwrap_or("").to_string();
        if name == "file" {
            filename = field.file_name().unwrap_or("unknown").to_string();
            let (bids, size) =
                stream_file_into_blocks(state.block_store.clone(), &mut field).await?;
            block_ids = bids;
            total_size = size;
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
        .cloned()
        .unwrap_or_else(|| info.parent_dir.clone());
    let relative_path = fields.get("relative_path").cloned().unwrap_or_default();
    let target_dir = compute_scoped_target_dir(&info, &parent_dir, &relative_path)?;

    if !block_ids.is_empty() {
        let uid = Some(info.user_id);
        let fs_id = state
            .file_service()
            .upload_file_committed_stream(
                &info.repo_id,
                &target_dir,
                &filename,
                block_ids,
                total_size,
                &info.username,
                uid,
                true,
                None,
            )
            .await?;
        return Ok(Json(
            json!([{"id": fs_id, "name": filename, "size": total_size}]),
        ));
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

    // Re-check write permission: the token was issued against the caller's
    // membership, but that may have been revoked since (matches download).
    crate::domain::permission::check_repo_write_permission(
        state.repos.member.as_ref(),
        &info.repo_id,
        info.user_id,
    )
    .await?;

    let boundary = ct
        .split("boundary=")
        .nth(1)
        .map(|s| s.trim().trim_matches('"').to_string())
        .ok_or_else(|| AppError::BadRequest("missing boundary".into()))?;
    let mut multipart = multer::Multipart::new(req.into_body().into_data_stream(), boundary);

    let mut fields: HashMap<String, String> = HashMap::new();
    let mut filename = String::new();
    let mut block_ids: Vec<String> = Vec::new();
    let mut total_size: i64 = 0;

    while let Some(mut field) = multipart
        .next_field()
        .await
        .map_err(|e| AppError::Internal(format!("multipart error: {e}")))?
    {
        let name = field.name().unwrap_or("").to_string();
        if name == "file" {
            filename = field.file_name().unwrap_or("unknown").to_string();
            let (bids, size) =
                stream_file_into_blocks(state.block_store.clone(), &mut field).await?;
            block_ids = bids;
            total_size = size;
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

    let target_file = fields.get("target_file").cloned().unwrap_or_default();
    let relative_path = fields.get("relative_path").cloned().unwrap_or_default();

    if !block_ids.is_empty() {
        let uid = Some(info.user_id);
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

            let fs_id = state
                .file_service()
                .upload_file_committed_stream(
                    &info.repo_id,
                    &target_dir,
                    &name,
                    block_ids,
                    total_size,
                    &info.username,
                    uid,
                    false,
                    None,
                )
                .await?;

            return Ok(Json(
                json!([{"id": fs_id, "name": name, "size": total_size}]),
            ));
        }

        // Fallback: parent_dir + relative_path + filename
        let parent_dir = fields.get("parent_dir").cloned().unwrap_or(info.parent_dir);
        let target_dir = compute_target_dir(&parent_dir, &relative_path)?;

        if !filename.is_empty() {
            let fs_id = state
                .file_service()
                .upload_file_committed_stream(
                    &info.repo_id,
                    &target_dir,
                    &filename,
                    block_ids,
                    total_size,
                    &info.username,
                    uid,
                    true,
                    None,
                )
                .await?;
            return Ok(Json(
                json!([{"id": fs_id, "name": filename, "size": total_size}]),
            ));
        }
    }

    Ok(ok_json())
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
    headers: HeaderMap,
    body: axum::body::Body,
) -> Result<Json<serde_json::Value>, AppError> {
    let info = state
        .token_manager
        .validate(&token)
        .ok_or_else(|| AppError::BadRequest("invalid or expired update token".into()))?;

    if info.op != "update" {
        return Err(AppError::BadRequest("token not valid for update".into()));
    }

    // Re-check write permission: the token was issued against the caller's
    // membership, but that may have been revoked since (matches download).
    crate::domain::permission::check_repo_write_permission(
        state.repos.member.as_ref(),
        &info.repo_id,
        info.user_id,
    )
    .await?;

    let boundary = extract_multipart_boundary(&headers)?;
    let mut multipart = multer::Multipart::new(body.into_data_stream(), boundary);

    let mut fields: HashMap<String, String> = HashMap::new();
    let mut block_ids: Vec<String> = Vec::new();
    let mut total_size: i64 = 0;

    while let Some(mut field) = multipart
        .next_field()
        .await
        .map_err(|e| AppError::Internal(format!("multipart error: {e}")))?
    {
        let name = field.name().unwrap_or("").to_string();
        if name == "file" {
            let (bids, size) =
                stream_file_into_blocks(state.block_store.clone(), &mut field).await?;
            block_ids = bids;
            total_size = size;
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

    if block_ids.is_empty() {
        return Ok(ok_json());
    }

    let uid = Some(info.user_id);
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

        let fs_id = state
            .file_service()
            .upload_file_committed_stream(
                &info.repo_id,
                &target_dir,
                &name,
                block_ids,
                total_size,
                &info.username,
                uid,
                false,
                None,
            )
            .await?;

        return Ok(Json(
            json!([{"id": fs_id, "name": name, "size": total_size}]),
        ));
    }

    Ok(ok_json())
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

    // Re-check write permission: the token was issued against the caller's
    // membership, but that may have been revoked since (matches download).
    crate::domain::permission::check_repo_write_permission(
        state.repos.member.as_ref(),
        &info.repo_id,
        info.user_id,
    )
    .await?;

    let uid = Some(info.user_id);
    let mut fields: HashMap<String, String> = HashMap::new();

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
                // Stream each block: verify its SHA-1 and write it immediately,
                // so one request can't buffer every block in memory at once.
                let computed = infra::crypto::fs_id::sha1_hex(&data);
                if computed != block_id {
                    return Err(AppError::BadRequest(format!(
                        "block ID mismatch: expected {block_id}, computed {computed}"
                    )));
                }
                state
                    .block_store
                    .write_block_with_id(&block_id, &data)
                    .await
                    .map_err(|e| {
                        AppError::Internal(format!("failed to write block {block_id}: {e}"))
                    })?;
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
        // Align with seafile's should_ignore_file: reject empty names, path
        // separators, NUL, "." / "..", and over-long names. Deliberately NOT
        // the strict character blacklist so clients can upload files with
        // quotes, brackets etc.
        if !is_valid_upload_filename(file_name) {
            return Err(AppError::BadRequest("invalid file_name".into()));
        }
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

        // Verify all blocks exist in block store and sum their real sizes.
        // Reject malformed / path-traversal ids outright — a valid block id
        // is exactly 40 hex chars (content-addressed SHA-1).
        if let Some(bad) = block_ids
            .iter()
            .find(|bid| bid.len() != 40 || !bid.bytes().all(|b| b.is_ascii_hexdigit()))
        {
            return Err(AppError::BadRequest(format!("invalid block id: {bad}")));
        }
        // Stat the blocks concurrently; order does not matter for a sum.
        use futures::StreamExt;
        let sizes: Vec<Result<i64, AppError>> = futures::stream::iter(block_ids.iter().cloned())
            .map(|bid| {
                let store = state.block_store.clone();
                async move {
                    store
                        .block_size(&bid)
                        .await
                        .map_err(|_| AppError::BadRequest(format!("block not found: {bid}")))
                }
            })
            .buffered(8)
            .collect()
            .await;
        let mut real_size: i64 = 0;
        for sz in sizes {
            real_size += sz?;
        }

        // The declared size must match the real block bytes, so a client
        // can't low-ball `file_size` to dodge the quota check.
        if real_size != file_size {
            return Err(AppError::BadRequest(
                "file_size does not match the uploaded blocks".into(),
            ));
        }

        // Pre-check storage quota against the actual block bytes before
        // assembling the file from its blocks.
        crate::service::fs::quota::check_upload_quota(
            &state.repos,
            info.user_id,
            real_size,
            state.config.storage.max_storage_bytes,
        )
        .await?;

        // Create FsFileData from block IDs
        let file_fs_data = base::common::FsFileData {
            block_ids: block_ids.clone(),
            size: file_size,
            obj_type: 1,
            version: 1,
        };
        let file_fs_id =
            crate::fs::core::store_fs_file_object(state.db.as_ref(), &info.repo_id, &file_fs_data)
                .await?;

        // Update directory tree and create commit
        let relative_path = fields
            .get("relative_path")
            .map(|s| s.as_str())
            .unwrap_or("");
        let target_dir = compute_target_dir(parent_dir, relative_path)?;
        let now = chrono::Utc::now().timestamp();

        // Full path of the assembled file (for activity logging).
        let fp = base::sanitize::safe_join_path(&target_dir, file_name)
            .map_err(|e| AppError::BadRequest(format!("invalid path: {e}")))?;

        // Resolve parent directory and capture ancestor chain for the
        // subsequent walk_up_ancestors (avoids O(d²) re-resolution).
        let (parent_fs_id, ancestor_chain) =
            crate::fs::core::file_ops::FileOps::resolve_fs_id_chain(
                &state.repos,
                &info.repo_id,
                &target_dir,
            )
            .await
            .map_err(|e| AppError::Internal(format!("resolve parent dir failed: {e}")))?;

        // Determine the pre-commit state of the target entry from the parent
        // dirents (avoids a fresh head-commit path resolution in finalize_upload).
        let parent_data =
            crate::fs::core::read_fs_dir_data(&state.repos, &info.repo_id, &parent_fs_id)
                .await
                .map_err(|e| AppError::Internal(format!("read parent dir failed: {e}")))?;
        let existing = parent_data.dirents.iter().find(|d| d.name == *file_name);
        let file_exists = existing.is_some();
        let old_size = existing.map(|d| d.size).unwrap_or(0);

        // Add file entry to parent directory
        let entry_name = file_name.clone();
        let modifier_name = info.username.clone();
        crate::fs::core::file_ops::FileOps::update_dir_tree_and_commit(
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
                        infra::common::util::generate_unique_filename(dirents, &entry_name);
                    dirents.push(base::common::DirEntryData {
                        id: file_fs_id.clone(),
                        mode: infra::serialization::S_IFREG,
                        modifier: modifier_name.clone(),
                        mtime: now,
                        name: unique_name,
                        size: file_size,
                    });
                } else {
                    dirents.push(base::common::DirEntryData {
                        id: file_fs_id.clone(),
                        mode: infra::serialization::S_IFREG,
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

        // Adjust repo size and log activity (op_type from actual existence).
        state
            .file_service()
            .finalize_upload(
                &info.repo_id,
                &fp,
                &file_fs_id,
                file_size,
                uid,
                file_exists,
                old_size,
            )
            .await?;

        return Ok(Json(json!({"id": file_fs_id})));
    }

    Ok(ok_json())
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
