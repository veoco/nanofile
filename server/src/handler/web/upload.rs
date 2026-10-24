use axum::{
    Json,
    extract::{Multipart, Path, State},
    http::HeaderMap,
    response::IntoResponse,
};
use serde_json::json;
use std::collections::HashMap;
use std::sync::Arc;

use crate::AppState;
use crate::handler::{
    MAX_BLOCK_UPLOAD_BYTES, MAX_SMALL_BODY_BYTES, ok_json, read_multipart_field_limited,
};
use crate::service::sharing::link as upload_link_service;
use crate::ui::auth_extractor::WebUser;
use base::error::AppError;

/// Compute the actual target directory from the base `parent_dir` and an
/// optional `relative_path` (e.g. `"myfolder/sub/"` from a folder upload).
///
/// Returns `AppError::BadRequest` if the result contains traversal components
/// escaping the repository root, or invalid characters.
fn compute_target_dir(parent_dir: &str, relative_path: &str) -> Result<String, AppError> {
    base::sanitize::safe_join_path(parent_dir, relative_path).map_err(|e| {
        AppError::BadRequest(format!(
            "Invalid path: {}. Please ensure the path does not contain '..' components that would escape the repository.",
            e
        ))
    })
}

/// Resolve the target directory for a token-authenticated upload, scoping
/// upload-link tokens to the directory the link was created for; a link must
/// never write outside it via a client-supplied `parent_dir` or `relative_path`.
fn compute_scoped_target_dir(
    info: &crate::service::auth::access_token::AccessToken,
    client_parent_dir: &str,
    relative_path: &str,
) -> Result<String, AppError> {
    if info.upload_link_id.is_some() {
        // Upload links are scoped to their directory: resolve relative to the
        // link's own path and clamp to it (`safe_join_path` only clamps to the
        // repo root).
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

/// Parse seafile's `replace` argument from the URL query (the `upload-link`
/// endpoint appends `?replace=1`) or a multipart form field, as upstream's
/// `r.FormValue("replace")` does. Absent, `0` or `false` keeps both (the new
/// file becomes `name (1).ext`); `1`/`true` overwrites in place; anything else
/// is rejected, matching `server/fileserver/fileop.go:1220-1231`.
fn parse_replace_arg(query: Option<&str>, form: Option<&str>) -> Result<bool, AppError> {
    let raw = form.or(query).map(str::trim).filter(|v| !v.is_empty());
    match raw {
        None | Some("0") | Some("false") => Ok(false),
        Some("1") | Some("true") => Ok(true),
        Some(other) => Err(AppError::BadRequest(format!(
            "Invalid argument replace: {other}"
        ))),
    }
}

/// Collect a URL's query parameters into a map (missing/unparsable → empty).
fn query_params(uri: &axum::http::Uri) -> HashMap<String, String> {
    uri.query()
        .and_then(|q| serde_urlencoded::from_str(q).ok())
        .unwrap_or_default()
}

/// True when the request carries `ret-json` in its query or form body. Upstream
/// tests the key's mere presence (`r.Form["ret-json"]`), so `ret-json=1`,
/// `ret-json=true` and a bare `ret-json` all switch the success body to JSON.
fn wants_ret_json(query: &HashMap<String, String>, fields: &HashMap<String, String>) -> bool {
    query.contains_key("ret-json") || fields.contains_key("ret-json")
}

/// Reject an update whose target does not exist with 441
/// (`SEAF_HTTP_RES_NOT_EXISTS`): upstream's update endpoints answer that instead
/// of silently creating the file (`server/fileserver/fileop.go:3339-3343`).
async fn require_target_exists(
    state: &AppState,
    repo_id: &str,
    target_dir: &str,
    name: &str,
) -> Result<(), AppError> {
    let path = base::sanitize::safe_join_path(target_dir, name)
        .map_err(|e| AppError::BadRequest(format!("invalid path: {e}")))?;
    if crate::fs::core::get_entry_total_size(&state.repos, repo_id, &path)
        .await
        .is_err()
    {
        return Err(AppError::NotExists);
    }
    Ok(())
}

/// Frame a committed upload's success body: `-aj` endpoints always answer the
/// JSON array (`isAjax=true`), while `-api` endpoints answer the plain-text file
/// id unless `ret-json` was requested — the official Android and desktop clients
/// store that body verbatim, so a JSON array there corrupts their state
/// (`server/fileserver/fileop.go:1859-1884`).
fn upload_commit_response(
    ret_json: bool,
    id: &str,
    name: &str,
    size: i64,
) -> axum::response::Response {
    use axum::response::IntoResponse;

    if ret_json {
        return Json(json!([{ "id": id, "name": name, "size": size }])).into_response();
    }
    (
        [(
            axum::http::header::CONTENT_TYPE,
            "text/plain; charset=utf-8",
        )],
        id.to_string(),
    )
        .into_response()
}

/// Outcome of a resumable (`Content-Range`) chunk request.
enum ChunkResult {
    /// Not the final chunk: upstream always answers `{"success": true}`.
    Intermediate,
    Committed {
        id: String,
        name: String,
        size: i64,
    },
}

/// Render a chunk outcome for the `-aj` endpoints, which always answer JSON.
fn chunk_result_json(result: ChunkResult) -> Json<serde_json::Value> {
    match result {
        ChunkResult::Intermediate => Json(json!({ "success": true })),
        ChunkResult::Committed { id, name, size } => {
            Json(json!([{ "id": id, "name": name, "size": size }]))
        }
    }
}

/// Validate an upload-link commit `file_name` like seafile's `should_ignore_file`:
/// reject empty names, path separators, NUL, "." / "..", and names over 255
/// bytes, rather than using a strict character blacklist.
fn is_valid_upload_filename(name: &str) -> bool {
    !name.is_empty()
        && name != "."
        && name != ".."
        && !name.contains('/')
        && !name.contains('\0')
        && name.len() <= 255
}

/// Pre-check storage quota against Content-Length before consuming the body. The
/// length is an upper bound (it includes multipart framing), so a pass does not
/// guarantee the post-commit check, but a fail rejects without writing blocks.
async fn precheck_quota(
    state: &AppState,
    user_id: Option<i32>,
    content_length: Option<&str>,
) -> Result<(), AppError> {
    let Some(uid) = user_id else {
        return Ok(());
    };
    let Some(len) = content_length
        .and_then(|s| s.parse::<i64>().ok())
        .filter(|&n| n > 0)
    else {
        return Ok(());
    };
    crate::service::fs::quota::check_upload_quota(
        &state.repos,
        uid,
        len,
        state.config().storage.default_user_quota_bytes,
    )
    .await
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

/// Read a chunked upload's file field into memory under the per-chunk cap.
///
/// Rejects an oversized chunk from the Content-Range header before reading any
/// body, and caps the bytes actually read so a client that lies about the header
/// cannot force more than `max_chunk_bytes` to be buffered (`field.bytes()` would
/// read the whole part first).
async fn read_chunked_field(
    field: &mut multer::Field<'_>,
    content_range: Option<&str>,
    max_chunk_bytes: u64,
) -> Result<Vec<u8>, AppError> {
    let Some(header) = content_range else {
        // Only reached when a Content-Range header exists but failed to parse
        // as UTF-8; reject rather than read the field unbounded.
        return Err(AppError::BadRequest("invalid content-range header".into()));
    };
    let (start, end, _file_size) = parse_content_range(header)?;
    let expected_len = (end - start + 1) as usize;
    if max_chunk_bytes > 0 && expected_len as u64 > max_chunk_bytes {
        return Err(AppError::BadRequest(format!(
            "chunk size {expected_len} exceeds per-chunk limit {max_chunk_bytes}"
        )));
    }
    // Enforce the cap on bytes actually read (chunk() may return one large Bytes
    // when the body is pre-buffered); an initial 1 MiB reserve stops a lying
    // header from forcing a large upfront allocation.
    let mut out = Vec::with_capacity(expected_len.min(1 << 20));
    while let Some(c) = field
        .chunk()
        .await
        .map_err(|e| AppError::Internal(format!("file read error: {e}")))?
    {
        if max_chunk_bytes > 0 && (out.len() + c.len()) as u64 > max_chunk_bytes {
            return Err(AppError::BadRequest(format!(
                "chunk exceeds per-chunk limit {max_chunk_bytes}"
            )));
        }
        out.extend_from_slice(&c);
    }
    Ok(out)
}

/// Map temp-file I/O errors onto HTTP errors: quota caps and out-of-range
/// chunk writes are client errors (429/400), everything else is a 500.
fn map_temp_error(e: std::io::Error) -> AppError {
    match e.kind() {
        std::io::ErrorKind::QuotaExceeded => AppError::TooManyRequests,
        std::io::ErrorKind::InvalidInput => AppError::BadRequest(e.to_string()),
        _ => AppError::Internal(format!("temp file operation failed: {e}")),
    }
}

/// Handle a chunked (resumable) upload when a `Content-Range` header is present.
///
/// `Ok(None)` means not chunked (the caller handles it as a regular upload);
/// `Intermediate` means a chunk was stored; `Committed` means the final chunk was
/// assembled and committed, and the caller frames the body for its endpoint.
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
    enc_key: Option<(&[u8], &[u8])>,
    replace: bool,
) -> Result<Option<ChunkResult>, AppError> {
    let Some(range_header) = content_range else {
        return Ok(None); // not a chunked upload
    };

    let (start, end, file_size) = parse_content_range(range_header)?;

    let expected_len = (end - start + 1) as usize;
    if file_data.len() != expected_len {
        return Err(AppError::BadRequest(format!(
            "Content-Range chunk size mismatch: header says {expected_len}, actual {}",
            file_data.len()
        )));
    }

    // Cap a single chunk so one request cannot buffer arbitrarily many bytes.
    let max_chunk_bytes = state.config().server.max_chunk_size_mb * 1024 * 1024;
    if max_chunk_bytes > 0 && expected_len as u64 > max_chunk_bytes {
        return Err(AppError::BadRequest(format!(
            "chunk size {expected_len} exceeds per-chunk limit {max_chunk_bytes}"
        )));
    }

    // Check total file size against server limit. 442 is seafile's
    // `SEAF_HTTP_RES_TOO_LARGE`, which the clients recognise; 400/413 are not.
    let max_bytes = state.config().server.max_upload_size_mb * 1024 * 1024;
    if max_bytes > 0 && file_size > max_bytes {
        return Err(AppError::TooLarge);
    }

    // Pre-check storage quota once, on the first chunk, against the declared
    // total size; the final assembly path re-checks it as a backstop.
    let file_path = base::sanitize::safe_join_path(target_dir, file_name)
        .map_err(|e| AppError::BadRequest(format!("invalid path: {e}")))?;

    temp_mgr
        .get_or_create(repo_id, &file_path, file_size)
        .await
        .map_err(map_temp_error)?;
    if let Some(uid) = user_id {
        temp_mgr.bind_owner(repo_id, &file_path, uid).await;
    }

    // Reserve (not just compare) the declared size so an upload abandoned before
    // its final chunk cannot leave quota-exempt blocks behind; the reservation is
    // released on commit, abort, or stale reaping.
    if start == 0
        && let Some(uid) = user_id
    {
        crate::service::fs::quota::reserve_block_bytes(
            &state.repos,
            uid,
            repo_id,
            file_size as i64,
            state.config().storage.default_user_quota_bytes,
        )
        .await?;
    }

    temp_mgr
        .write_chunk(repo_id, &file_path, start, file_data)
        .await
        .map_err(map_temp_error)?;

    // In-transit streaming: in-order chunks feed the CDC chunker so a fully
    // in-order upload commits on the final chunk; out-of-order chunks fall back
    // to temp-file assembly. Encrypted libraries always take the fallback: the
    // stream hashes plaintext (`sha1(blk)`), but they must store `sha1(ciphertext)`.
    if enc_key.is_none() {
        let _ = temp_mgr
            .feed_stream(&state.block_store, repo_id, &file_path, start, file_data)
            .await;
    }

    if end != file_size - 1 {
        return Ok(Some(ChunkResult::Intermediate));
    }

    // Fast path: a fully in-order upload already has its block ids persisted,
    // so commit directly and skip the temp-file re-read.
    if let Some((block_ids, total_size, new_block_ids)) = temp_mgr
        .take_streamed_blocks(&state.block_store, repo_id, &file_path, file_size)
        .await
    {
        let (fs_id, effective_name) = state
            .file_service()
            .upload_file_committed_stream(
                repo_id,
                target_dir,
                file_name,
                block_ids,
                total_size,
                modifier,
                user_id,
                true,
                replace,
                new_block_ids,
            )
            .await?;
        temp_mgr.finish(repo_id, &file_path).await;
        return Ok(Some(ChunkResult::Committed {
            id: fs_id,
            name: effective_name,
            size: total_size,
        }));
    }

    // Fallback path: stream the temp file through the CDC chunker so the whole
    // file is never buffered in memory, then commit the resulting block ids.
    let Some(stream) = temp_mgr.read_stream(repo_id, &file_path).await else {
        temp_mgr
            .abort(&state.repos, repo_id, &file_path, &state.block_store)
            .await;
        return Err(AppError::Internal(
            "failed to open assembled temp file".into(),
        ));
    };

    let (block_ids, total_size, new_block_ids) = crate::fs::core::FileOps::write_stream_blocks(
        &state.block_store,
        repo_id,
        file_size as usize,
        stream,
        enc_key,
    )
    .await?;

    // Track written blocks so an abort or stale cleanup can delete them.
    temp_mgr
        .record_new_blocks(repo_id, &file_path, &new_block_ids)
        .await;

    // The streamed total must match the declared file size.
    if total_size as u64 != file_size {
        temp_mgr
            .abort(&state.repos, repo_id, &file_path, &state.block_store)
            .await;
        return Err(AppError::Internal(format!(
            "assembled file size {total_size} does not match expected {file_size}"
        )));
    }

    let (fs_id, effective_name) = state
        .file_service()
        .upload_file_committed_stream(
            repo_id,
            target_dir,
            file_name,
            block_ids,
            total_size,
            modifier,
            user_id,
            true,
            replace,
            new_block_ids,
        )
        .await?;

    temp_mgr.finish(repo_id, &file_path).await;

    Ok(Some(ChunkResult::Committed {
        id: fs_id,
        name: effective_name,
        size: total_size,
    }))
}

/// Simple multipart/form-data parser that finds a named field's value and
/// extracts the file (if any) from the body.
///
/// Used instead of axum's `Multipart` extractor, which fails with 400 on the Qt
/// desktop client's uploads.
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
        // The first boundary has no leading CRLF.
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
        if boundary_end + 2 <= data.len() && &data[boundary_end..boundary_end + 2] == b"--" {
            break; // closing boundary
        }
        if boundary_end + 2 <= data.len() && &data[boundary_end..boundary_end + 2] == b"\r\n" {
            boundary_end += 2;
        }
        pos = boundary_end;

        if let Some(hdr_end) = data[pos..].windows(4).position(|w| w == b"\r\n\r\n") {
            let hdr = String::from_utf8_lossy(&data[pos..pos + hdr_end]);
            pos += hdr_end + 4;

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

            // Trim the trailing CRLF that belongs to the boundary.
            let body = body
                .strip_suffix(b"\r\n")
                .or_else(|| body.strip_suffix(b"\n"))
                .unwrap_or(body);

            let field_name = hdr
                .split(';')
                .find_map(|s| s.trim().strip_prefix("name=\"")?.split('"').next())
                .unwrap_or("");

            // Extract filename — handle both single-line and multi-line headers.
            // Qt sends `filename="value"\r\nContent-Type: ...`, so split on `"`
            // rather than using strip_suffix('"').
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

/// `POST /upload-aj/` — AJAX file upload from the Seahub web UI (no token),
/// taking `file`, `repo_id`, `parent_dir` and optional `relative_path` fields.
pub async fn upload_aj(
    user: WebUser,
    State(state): State<Arc<AppState>>,
    uri: axum::http::Uri,
    headers: HeaderMap,
    body: axum::body::Body,
) -> Result<Json<serde_json::Value>, AppError> {
    // Pre-check quota against Content-Length before consuming the body, so an
    // over-quota upload is rejected without writing blocks; chunked uploads are
    // pre-checked on their first chunk instead.
    if headers.get("content-range").is_none() {
        precheck_quota(
            &state,
            Some(user.user_id),
            headers.get("content-length").and_then(|v| v.to_str().ok()),
        )
        .await?;
    }
    let boundary = extract_multipart_boundary(&headers)?;
    // Multer's Multipart consumes the raw body stream, so the file part can be
    // read incrementally via `field.chunk()` (axum's extractor offers only
    // whole-field `bytes()`).
    let mut multipart = multer::Multipart::new(body.into_data_stream(), boundary);

    let mut fields: HashMap<String, String> = HashMap::new();
    let mut filename = String::new();

    // Resumable uploads need the whole chunk in hand, so they keep the buffered read.
    let content_range = headers.get("content-range").and_then(|v| v.to_str().ok());
    let is_chunked = content_range.is_some();
    let mut chunked_file_data: Option<Vec<u8>> = None;
    // Non-chunked uploads are staged on disk, ingested only after authorization.
    let mut staged_path: Option<std::path::PathBuf> = None;
    let staging_dir = upload_staging_dir(&state);

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
                    read_chunked_field(
                        &mut field,
                        content_range,
                        state.config().server.max_chunk_size_mb * 1024 * 1024,
                    )
                    .await?,
                );
            } else {
                let (path, _size) =
                    stage_file_field(&mut field, &staging_dir, max_upload_bytes(&state)).await?;
                staged_path = Some(path);
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
    let query = query_params(&uri);
    let replace = parse_replace_arg(
        query.get("replace").map(String::as_str),
        fields.get("replace").map(String::as_str),
    )?;

    // CSRF: this endpoint authenticates via the session cookie, so a
    // cross-site form POST must be rejected.
    crate::service::auth::csrf::check_form_csrf(
        &state,
        &user.session_token,
        fields.get("csrf_token").map(|s| s.as_str()),
    )?;

    // Authorization: this endpoint trusts the client-supplied repo_id, so the
    // caller must actually be a member with write access to the repo.
    crate::domain::permission::check_repo_write_permission(
        state.repos.member.as_ref(),
        repo_id,
        user.user_id,
    )
    .await?;

    // Encrypted libraries require the password-equivalent key before anything
    // reaches the block store; staged bytes are plaintext in the staging area,
    // which is not part of the library.
    let enc_key = super::download::upload_block_key(&state, repo_id, user.user_id, false).await?;
    let enc_key_ref = enc_key.as_ref().map(|(k, i)| (k.as_slice(), i.as_slice()));

    if is_chunked {
        let file_data = chunked_file_data.unwrap_or_default();
        if !file_data.is_empty()
            && let Some(result) = try_handle_chunked(
                &state.temp_file_manager,
                &state,
                repo_id,
                &target_dir,
                &filename,
                &file_data,
                content_range,
                &user.email,
                Some(user.user_id),
                enc_key_ref,
                replace,
            )
            .await?
        {
            return Ok(chunk_result_json(result));
        }
        return Ok(Json(json!([{"name": filename, "uploaded": true}])));
    }

    // Ingest the staged bytes now that the caller is authorized; the guard
    // removes the staging file on every exit path.
    let staged = StagedUpload(staged_path);
    if let Some(path) = staged.path() {
        let ingested = ingest_staged_file(
            state.block_store.clone(),
            repo_id,
            path.to_path_buf(),
            enc_key_ref,
        )
        .await;
        let (block_ids, total_size, new_block_ids) = ingested?;
        // Zero-byte files are committed too: seafile gives them the `EMPTY_SHA1`
        // sentinel id but still creates the dirent (`server/fileserver/fileop.go:3699-3702`).
        let (fs_id, effective_name) = state
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
                replace,
                new_block_ids,
            )
            .await?;
        return Ok(Json(
            json!([{"id": fs_id, "name": effective_name, "size": total_size}]),
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
/// block store, so the file is never fully buffered in memory; `Chunker::new(0)`
/// uses seafile's fixed 8 MiB avg / 6 MiB min / 10 MiB max sizes.
///
/// `enc_key` is the library block key for an encrypted repository (see
/// `upload_block_key`); when set, each block is encrypted before it is stored
/// and its id becomes `sha1(ciphertext)`, the Seafile convention
/// (`common/fs-mgr.c:707-721` hashes the encrypted buffer).
pub(crate) async fn stream_file_into_blocks(
    store: infra::storage::DynBlockStorage,
    repo_id: &str,
    field: &mut multer::Field<'_>,
    enc_key: Option<(&[u8], &[u8])>,
) -> Result<(Vec<String>, i64, Vec<String>), AppError> {
    crate::fs::core::FileOps::stream_blocks_pipelined(
        &store,
        repo_id,
        enc_key,
        move |tx| async move {
            let mut chunker = infra::storage::cdc::Chunker::new(0);
            let mut total_size = 0i64;
            let mut idx = 0usize;
            while let Some(c) = field
                .chunk()
                .await
                .map_err(|e| AppError::Internal(format!("file read error: {e}")))?
            {
                for block in chunker.feed(&c) {
                    total_size += block.len() as i64;
                    tx.send((idx, block))
                        .await
                        .map_err(|_| AppError::Internal("block writer stopped".into()))?;
                    idx += 1;
                }
            }
            let last = chunker.finish();
            if !last.is_empty() {
                total_size += last.len() as i64;
                tx.send((idx, last))
                    .await
                    .map_err(|_| AppError::Internal("block writer stopped".into()))?;
            }
            Ok(total_size)
        },
    )
    .await
}

/// A multipart file staged on disk before authorization.
///
/// The no-token web endpoints carry the repository id inside the multipart body,
/// so permission can only be checked after parsing; staging the bytes keeps them
/// out of the global block store until then, and the guard removes the file on
/// drop.
struct StagedUpload(Option<std::path::PathBuf>);

impl StagedUpload {
    fn path(&self) -> Option<&std::path::Path> {
        self.0.as_deref()
    }
}

impl Drop for StagedUpload {
    fn drop(&mut self) {
        if let Some(path) = self.0.take() {
            let _ = std::fs::remove_file(path);
        }
    }
}

/// Stream a multipart file field into a request-scoped staging file.
async fn stage_file_field(
    field: &mut multer::Field<'_>,
    staging_dir: &std::path::Path,
    max_bytes: u64,
) -> Result<(std::path::PathBuf, i64), AppError> {
    use tokio::io::AsyncWriteExt;

    tokio::fs::create_dir_all(staging_dir)
        .await
        .map_err(|e| AppError::Internal(format!("create staging dir: {e}")))?;
    let path = staging_dir.join(uuid::Uuid::new_v4().to_string());
    let mut file = tokio::fs::File::create(&path)
        .await
        .map_err(|e| AppError::Internal(format!("create staging file: {e}")))?;

    let mut size: i64 = 0;
    while let Some(chunk) = field
        .chunk()
        .await
        .map_err(|e| AppError::Internal(format!("file read error: {e}")))?
    {
        size += chunk.len() as i64;
        if max_bytes > 0 && size as u64 > max_bytes {
            // 442, the code the clients map to "file too large" and abort on.
            return Err(AppError::TooLarge);
        }
        file.write_all(&chunk)
            .await
            .map_err(|e| AppError::Internal(format!("write staging file: {e}")))?;
    }
    file.flush()
        .await
        .map_err(|e| AppError::Internal(format!("flush staging file: {e}")))?;
    Ok((path, size))
}

/// CDC a staged file into the block store (called only after authorization).
///
/// `enc_key` behaves exactly as in [`stream_file_into_blocks`].
async fn ingest_staged_file(
    store: infra::storage::DynBlockStorage,
    repo_id: &str,
    path: std::path::PathBuf,
    enc_key: Option<(&[u8], &[u8])>,
) -> Result<(Vec<String>, i64, Vec<String>), AppError> {
    use tokio::io::AsyncReadExt;

    crate::fs::core::FileOps::stream_blocks_pipelined(
        &store,
        repo_id,
        enc_key,
        move |tx| async move {
            let mut file = tokio::fs::File::open(&path)
                .await
                .map_err(|e| AppError::Internal(format!("open staged file: {e}")))?;
            let mut chunker = infra::storage::cdc::Chunker::new(0);
            let mut total_size = 0i64;
            let mut idx = 0usize;
            let mut buf = vec![0u8; 64 * 1024];
            loop {
                let n = file
                    .read(&mut buf)
                    .await
                    .map_err(|e| AppError::Internal(format!("read staged file: {e}")))?;
                if n == 0 {
                    break;
                }
                for block in chunker.feed(&buf[..n]) {
                    total_size += block.len() as i64;
                    tx.send((idx, block))
                        .await
                        .map_err(|_| AppError::Internal("block writer stopped".into()))?;
                    idx += 1;
                }
            }
            let last = chunker.finish();
            if !last.is_empty() {
                total_size += last.len() as i64;
                tx.send((idx, last))
                    .await
                    .map_err(|_| AppError::Internal("block writer stopped".into()))?;
            }
            Ok(total_size)
        },
    )
    .await
}

/// Staging directory for uploads whose authorization happens after parsing.
fn upload_staging_dir(state: &AppState) -> std::path::PathBuf {
    state.config().storage.temp_dir.join("staging")
}

/// Maximum upload size in bytes (`0` = unlimited).
fn max_upload_bytes(state: &AppState) -> u64 {
    state.config().server.max_upload_size_mb * 1024 * 1024
}

/// `POST /update-api/` — update an existing file from the web UI (no token),
/// taking `file`, `repo_id`, and `p` / `path` (the full target path).
pub async fn update_api(
    user: WebUser,
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    body: axum::body::Body,
) -> Result<Json<serde_json::Value>, AppError> {
    // An update always replaces an existing file, so the bytes it frees are part
    // of the budget. A Content-Length precheck cannot know that delta and used to
    // reject a same-size replacement near the quota; the commit-time check in
    // `upload_file_committed_stream` applies the correct delta instead.
    let boundary = extract_multipart_boundary(&headers)?;
    let mut multipart = multer::Multipart::new(body.into_data_stream(), boundary);

    let mut repo_id = String::new();
    let mut file_path = String::new();
    let mut csrf_token: Option<String> = None;
    // Stage the bytes on disk; authorization happens after parsing.
    let mut staged_path: Option<std::path::PathBuf> = None;
    let staging_dir = upload_staging_dir(&state);

    while let Some(mut field) = multipart
        .next_field()
        .await
        .map_err(|e| AppError::Internal(format!("multipart error: {e}")))?
    {
        let name = field.name().unwrap_or("").to_string();
        if name == "file" {
            let (path, _size) =
                stage_file_field(&mut field, &staging_dir, max_upload_bytes(&state)).await?;
            staged_path = Some(path);
        } else {
            let val = field
                .text()
                .await
                .map_err(|e| AppError::Internal(format!("multipart field error: {e}")))?;
            if name == "repo_id" {
                repo_id = val.clone();
            } else if name == "csrf_token" {
                csrf_token = Some(val);
            } else if name == "p" || name == "path" {
                file_path = val;
            }
        }
    }

    let staged = StagedUpload(staged_path);

    if !file_path.is_empty() {
        // CSRF: these no-token endpoints authenticate via the session cookie,
        // so a cross-site form POST must be rejected.
        crate::service::auth::csrf::check_form_csrf(
            &state,
            &user.session_token,
            csrf_token.as_deref(),
        )?;

        // Authorization: repo_id is client-supplied, so require write access.
        crate::domain::permission::check_repo_write_permission(
            state.repos.member.as_ref(),
            &repo_id,
            user.user_id,
        )
        .await?;

        // Encrypted libraries need the cached key before blocks are written.
        let enc_key =
            super::download::upload_block_key(&state, &repo_id, user.user_id, false).await?;
        let enc_key_ref = enc_key.as_ref().map(|(k, i)| (k.as_slice(), i.as_slice()));

        let parent = file_path
            .rsplit_once('/')
            .map(|(p, _)| if p.is_empty() { "/" } else { p })
            .unwrap_or("/");
        let name = file_path
            .rsplit_once('/')
            .map(|(_, n)| n)
            .unwrap_or(&file_path);
        require_target_exists(&state, &repo_id, parent, name).await?;

        if let Some(path) = staged.path() {
            let ingested = ingest_staged_file(
                state.block_store.clone(),
                &repo_id,
                path.to_path_buf(),
                enc_key_ref,
            )
            .await;
            let (block_ids, total_size, new_block_ids) = ingested?;
            // Update semantics: overwrite in place, and commit zero-byte files too.
            let (fs_id, effective_name) = state
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
                    true,
                    new_block_ids,
                )
                .await?;
            return Ok(Json(
                json!([{"id": fs_id, "name": effective_name, "size": total_size}]),
            ));
        }
    }

    Ok(ok_json())
}

/// `POST /update-aj/` — AJAX file update from the Seahub web UI (no token),
/// taking `file`, `repo_id` and `target_file` (the full target path).
pub async fn update_aj(
    user: WebUser,
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    body: axum::body::Body,
) -> Result<Json<serde_json::Value>, AppError> {
    // See `update_api`: an update replaces the file, so only the commit-time
    // size delta is a correct quota check.
    let boundary = extract_multipart_boundary(&headers)?;
    let mut multipart = multer::Multipart::new(body.into_data_stream(), boundary);

    let mut fields: HashMap<String, String> = HashMap::new();
    let content_range = headers.get("content-range").and_then(|v| v.to_str().ok());
    let is_chunked = content_range.is_some();
    let mut chunked_file_data: Option<Vec<u8>> = None;
    // Non-chunked uploads are staged on disk until authorization passes.
    let mut staged_path: Option<std::path::PathBuf> = None;
    let staging_dir = upload_staging_dir(&state);

    while let Some(mut field) = multipart
        .next_field()
        .await
        .map_err(|e| AppError::Internal(format!("multipart error: {e}")))?
    {
        let name = field.name().unwrap_or("").to_string();
        if name == "file" {
            if is_chunked {
                chunked_file_data = Some(
                    read_chunked_field(
                        &mut field,
                        content_range,
                        state.config().server.max_chunk_size_mb * 1024 * 1024,
                    )
                    .await?,
                );
            } else {
                let (path, _size) =
                    stage_file_field(&mut field, &staging_dir, max_upload_bytes(&state)).await?;
                staged_path = Some(path);
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

    // CSRF: session-cookie auth requires the double-submit token.
    crate::service::auth::csrf::check_form_csrf(
        &state,
        &user.session_token,
        fields.get("csrf_token").map(|s| s.as_str()),
    )?;

    // Authorization: the repo_id is client-supplied, so require write access.
    crate::domain::permission::check_repo_write_permission(
        state.repos.member.as_ref(),
        repo_id,
        user.user_id,
    )
    .await?;

    // Encrypted libraries need the cached key before blocks are written.
    let enc_key = super::download::upload_block_key(&state, repo_id, user.user_id, false).await?;
    let enc_key_ref = enc_key.as_ref().map(|(k, i)| (k.as_slice(), i.as_slice()));

    let parent = target_file
        .rsplit_once('/')
        .map(|(p, _)| if p.is_empty() { "/" } else { p })
        .unwrap_or("/");
    let name = target_file
        .rsplit_once('/')
        .map(|(_, n)| n)
        .unwrap_or(target_file);
    require_target_exists(&state, repo_id, parent, name).await?;

    if is_chunked {
        let file_data = chunked_file_data.unwrap_or_default();
        if !file_data.is_empty()
            && let Some(result) = try_handle_chunked(
                &state.temp_file_manager,
                &state,
                repo_id,
                parent,
                name,
                &file_data,
                content_range,
                &user.email,
                Some(user.user_id),
                enc_key_ref,
                // The update endpoints always replace the target in place.
                true,
            )
            .await?
        {
            return Ok(chunk_result_json(result));
        }
        return Ok(ok_json());
    }

    let staged = StagedUpload(staged_path);
    if let Some(path) = staged.path() {
        let ingested = ingest_staged_file(
            state.block_store.clone(),
            repo_id,
            path.to_path_buf(),
            enc_key_ref,
        )
        .await;
        let (block_ids, total_size, new_block_ids) = ingested?;
        // Update semantics: overwrite in place, and commit zero-byte files too.
        let (fs_id, effective_name) = state
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
                true,
                new_block_ids,
            )
            .await?;
        return Ok(Json(
            json!([{"id": fs_id, "name": effective_name, "size": total_size}]),
        ));
    }

    Ok(ok_json())
}

/// `POST /upload-aj/{token}` — upload with an upload-link token, as sent by the
/// Seahub React frontend after `/api2/repos/{id}/upload-link/?from=web`.
pub async fn upload_aj_token(
    State(state): State<Arc<AppState>>,
    Path(token): Path<String>,
    uri: axum::http::Uri,
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

    // A shareable upload link has no password cache to draw on, so an encrypted
    // library is refused outright; every other caller must have warmed the
    // server-side library key (440 otherwise).
    let enc_key = super::download::upload_block_key(
        &state,
        &info.repo_id,
        info.user_id,
        info.upload_link_id.is_some(),
    )
    .await?;
    let enc_key_ref = enc_key.as_ref().map(|(k, i)| (k.as_slice(), i.as_slice()));

    if headers.get("content-range").is_none() {
        precheck_quota(
            &state,
            Some(info.user_id),
            headers.get("content-length").and_then(|v| v.to_str().ok()),
        )
        .await?;
    }
    let boundary = extract_multipart_boundary(&headers)?;
    let mut multipart = multer::Multipart::new(body.into_data_stream(), boundary);

    let mut fields: HashMap<String, String> = HashMap::new();
    let mut filename = String::new();
    let content_range = headers.get("content-range").and_then(|v| v.to_str().ok());
    let is_chunked = content_range.is_some();
    let mut chunked_file_data: Option<Vec<u8>> = None;
    let mut block_ids: Vec<String> = Vec::new();
    let mut new_block_ids: Vec<String> = Vec::new();
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
                    read_chunked_field(
                        &mut field,
                        content_range,
                        state.config().server.max_chunk_size_mb * 1024 * 1024,
                    )
                    .await?,
                );
            } else {
                let (bids, size, nids) = stream_file_into_blocks(
                    state.block_store.clone(),
                    &info.repo_id,
                    &mut field,
                    enc_key_ref,
                )
                .await?;
                block_ids = bids;
                new_block_ids = nids;
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
    let query = query_params(&uri);
    let replace = parse_replace_arg(
        query.get("replace").map(String::as_str),
        fields.get("replace").map(String::as_str),
    )?;

    if is_chunked {
        let file_data = chunked_file_data.unwrap_or_default();
        if !file_data.is_empty()
            && let Some(result) = try_handle_chunked(
                &state.temp_file_manager,
                &state,
                &info.repo_id,
                &target_dir,
                &filename,
                &file_data,
                content_range,
                &info.username,
                // Upload-link chunked uploads count against the owner's quota.
                Some(info.user_id),
                enc_key_ref,
                replace,
            )
            .await?
        {
            return Ok(chunk_result_json(result));
        }
        return Ok(Json(json!([{"name": filename, "uploaded": true}])));
    }

    let uid = Some(info.user_id);
    let (fs_id, effective_name) = state
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
            replace,
            new_block_ids,
        )
        .await?;

    if let Some(link_id) = info.upload_link_id {
        upload_link_service::increment_upload_view_cnt(state.repos.clone(), link_id);
    }

    Ok(Json(
        json!([{"id": fs_id, "name": effective_name, "size": total_size}]),
    ))
}

/// `POST /upload-api/{token}` — token-authenticated upload for the desktop
/// client, which sends a quoted multipart boundary that axum's extractor rejects.
pub async fn upload_api(
    State(state): State<Arc<AppState>>,
    Path(token): Path<String>,
    req: axum::http::Request<axum::body::Body>,
) -> Result<axum::response::Response, AppError> {
    let ct = req
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_owned())
        .unwrap_or_default();
    // Resumable uploads reach this endpoint: the Qt desktop client fetches
    // `api2/repos/{id}/upload-link/` and chunks files over 100 MB
    // (`ReliablePostFileTask`). A chunk must go through the temp-file / assembly
    // path — streaming it straight into blocks would commit it as a whole file,
    // and that path is also where the declared size is reserved against quota.
    let content_range = req
        .headers()
        .get("content-range")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_owned());
    let content_length = req
        .headers()
        .get("content-length")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_owned());
    let has_range = content_range.is_some();

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

    // Encrypted libraries need the cached library key before blocks are
    // written; a shareable upload link is refused (see `upload_block_key`).
    let enc_key = super::download::upload_block_key(
        &state,
        &info.repo_id,
        info.user_id,
        info.upload_link_id.is_some(),
    )
    .await?;
    let enc_key_ref = enc_key.as_ref().map(|(k, i)| (k.as_slice(), i.as_slice()));

    if !has_range {
        precheck_quota(&state, Some(info.user_id), content_length.as_deref()).await?;
    }

    // Qt's QHttpMultiPart sends a quoted boundary (`boundary="_.Seafile._UUID"`)
    // while the body uses the unquoted form, so strip surrounding quotes. The
    // body is consumed as a stream so the file is never buffered.
    let boundary = ct
        .split("boundary=")
        .nth(1)
        .map(|s| s.trim().trim_matches('"').to_string())
        .ok_or_else(|| AppError::BadRequest("missing boundary".into()))?;
    // The query has to be captured before the request body is consumed.
    let query = query_params(req.uri());
    let mut multipart = multer::Multipart::new(req.into_body().into_data_stream(), boundary);

    let mut fields: HashMap<String, String> = HashMap::new();
    let mut filename = String::new();
    let mut block_ids: Vec<String> = Vec::new();
    let mut new_block_ids: Vec<String> = Vec::new();
    let mut total_size: i64 = 0;
    let is_chunked = content_range.is_some();
    let mut chunked_file_data: Option<Vec<u8>> = None;

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
                    read_chunked_field(
                        &mut field,
                        content_range.as_deref(),
                        state.config().server.max_chunk_size_mb * 1024 * 1024,
                    )
                    .await?,
                );
            } else {
                let (bids, size, nids) = stream_file_into_blocks(
                    state.block_store.clone(),
                    &info.repo_id,
                    &mut field,
                    enc_key_ref,
                )
                .await?;
                block_ids = bids;
                new_block_ids = nids;
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
        .cloned()
        .unwrap_or_else(|| info.parent_dir.clone());
    let relative_path = fields.get("relative_path").cloned().unwrap_or_default();
    let target_dir = compute_scoped_target_dir(&info, &parent_dir, &relative_path)?;
    let replace = parse_replace_arg(
        query.get("replace").map(String::as_str),
        fields.get("replace").map(String::as_str),
    )?;
    // Only `/upload-api` honours `ret-json`; official Android/desktop clients
    // read the plain-text body as the file id, so the default stays plain text.
    let ret_json = wants_ret_json(&query, &fields);

    if is_chunked {
        let file_data = chunked_file_data.unwrap_or_default();
        if !file_data.is_empty()
            && let Some(result) = try_handle_chunked(
                &state.temp_file_manager,
                &state,
                &info.repo_id,
                &target_dir,
                &filename,
                &file_data,
                content_range.as_deref(),
                &info.username,
                Some(info.user_id),
                enc_key_ref,
                replace,
            )
            .await?
        {
            return Ok(match result {
                ChunkResult::Intermediate => Json(json!({ "success": true })).into_response(),
                ChunkResult::Committed { id, name, size } => {
                    upload_commit_response(ret_json, &id, &name, size)
                }
            });
        }
        return Ok(Json(json!([{"name": filename, "uploaded": true}])).into_response());
    }

    let uid = Some(info.user_id);
    let (fs_id, effective_name) = state
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
            replace,
            new_block_ids,
        )
        .await?;
    Ok(upload_commit_response(
        ret_json,
        &fs_id,
        &effective_name,
        total_size,
    ))
}

/// `POST /update-api/{token}` — token-authenticated file overwrite for the
/// desktop client. `target_file`, or `parent_dir` + `relative_path`, names the
/// existing file; the update always overwrites in place.
pub async fn update_api_handler(
    State(state): State<Arc<AppState>>,
    Path(token): Path<String>,
    req: axum::http::Request<axum::body::Body>,
) -> Result<axum::response::Response, AppError> {
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

    // Encrypted libraries need the cached library key before blocks are
    // written; a shareable upload link is refused (see `upload_block_key`).
    let enc_key = super::download::upload_block_key(
        &state,
        &info.repo_id,
        info.user_id,
        info.upload_link_id.is_some(),
    )
    .await?;
    let enc_key_ref = enc_key.as_ref().map(|(k, i)| (k.as_slice(), i.as_slice()));

    // See `update_api`: this endpoint only replaces an existing file, so the
    // commit-time delta is the only correct quota check.

    let boundary = ct
        .split("boundary=")
        .nth(1)
        .map(|s| s.trim().trim_matches('"').to_string())
        .ok_or_else(|| AppError::BadRequest("missing boundary".into()))?;
    // The query has to be captured before the request body is consumed.
    let query = query_params(req.uri());
    let mut multipart = multer::Multipart::new(req.into_body().into_data_stream(), boundary);

    let mut fields: HashMap<String, String> = HashMap::new();
    let mut filename = String::new();
    let mut block_ids: Vec<String> = Vec::new();
    let mut new_block_ids: Vec<String> = Vec::new();
    let mut total_size: i64 = 0;

    while let Some(mut field) = multipart
        .next_field()
        .await
        .map_err(|e| AppError::Internal(format!("multipart error: {e}")))?
    {
        let name = field.name().unwrap_or("").to_string();
        if name == "file" {
            filename = field.file_name().unwrap_or("unknown").to_string();
            let (bids, size, nids) = stream_file_into_blocks(
                state.block_store.clone(),
                &info.repo_id,
                &mut field,
                enc_key_ref,
            )
            .await?;
            block_ids = bids;
            new_block_ids = nids;
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
    // `/update-api` honours `ret-json`; without it the body is a plain id.
    let ret_json = wants_ret_json(&query, &fields);

    // No `block_ids.is_empty()` guard: a zero-byte update still has to commit
    // (its fs id is the `EMPTY_SHA1` sentinel).
    let uid = Some(info.user_id);
    if !target_file.is_empty() {
        let (raw_parent, raw_name) = target_file.rsplit_once('/').unwrap_or(("/", &target_file));
        let parent = if raw_parent.is_empty() {
            "/"
        } else {
            raw_parent
        };
        // Scoped for consistency: an update token never carries `upload_link_id`
        // today, but this keeps the link-directory clamp if one ever does.
        let target_dir = compute_scoped_target_dir(&info, parent, &relative_path)?;
        let name = raw_name.to_string();
        require_target_exists(&state, &info.repo_id, &target_dir, &name).await?;

        let (fs_id, effective_name) = state
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
                // The update endpoint always overwrites in place.
                true,
                new_block_ids,
            )
            .await?;

        return Ok(upload_commit_response(
            ret_json,
            &fs_id,
            &effective_name,
            total_size,
        ));
    }

    // Fallback: parent_dir + relative_path + filename
    let parent_dir = fields.get("parent_dir").cloned().unwrap_or(info.parent_dir);
    let target_dir = compute_target_dir(&parent_dir, &relative_path)?;

    if !filename.is_empty() {
        require_target_exists(&state, &info.repo_id, &target_dir, &filename).await?;
        let (fs_id, effective_name) = state
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
                true,
                new_block_ids,
            )
            .await?;
        return Ok(upload_commit_response(
            ret_json,
            &fs_id,
            &effective_name,
            total_size,
        ));
    }

    Ok(ok_json().into_response())
}

/// `POST /update-aj/{token}` — token-based AJAX file update from the Seahub web
/// frontend, taking `file`, `target_file` and optional `relative_path`.
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

    // Encrypted libraries need the cached library key before blocks are
    // written; a shareable upload link is refused (see `upload_block_key`).
    let enc_key = super::download::upload_block_key(
        &state,
        &info.repo_id,
        info.user_id,
        info.upload_link_id.is_some(),
    )
    .await?;
    let enc_key_ref = enc_key.as_ref().map(|(k, i)| (k.as_slice(), i.as_slice()));

    let boundary = extract_multipart_boundary(&headers)?;
    let mut multipart = multer::Multipart::new(body.into_data_stream(), boundary);

    let mut fields: HashMap<String, String> = HashMap::new();
    let mut block_ids: Vec<String> = Vec::new();
    let mut new_block_ids: Vec<String> = Vec::new();
    let mut total_size: i64 = 0;

    while let Some(mut field) = multipart
        .next_field()
        .await
        .map_err(|e| AppError::Internal(format!("multipart error: {e}")))?
    {
        let name = field.name().unwrap_or("").to_string();
        if name == "file" {
            let (bids, size, nids) = stream_file_into_blocks(
                state.block_store.clone(),
                &info.repo_id,
                &mut field,
                enc_key_ref,
            )
            .await?;
            block_ids = bids;
            new_block_ids = nids;
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

    // Update semantics: overwrite in place, and commit zero-byte files too.
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
        require_target_exists(&state, &info.repo_id, &target_dir, &name).await?;

        let (fs_id, effective_name) = state
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
                true,
                new_block_ids,
            )
            .await?;

        return Ok(Json(
            json!([{"id": fs_id, "name": effective_name, "size": total_size}]),
        ));
    }

    Ok(ok_json())
}

/// `POST /upload-blks-api/{token}` — block upload and commit.
///
/// Without `commitonly`, each `file` part is one block whose filename is its
/// SHA-1, verified before storing. With `commitonly`, the `parent_dir`,
/// `file_name`, `blockids` (JSON array), `file_size` and optional `replace`
/// fields assemble the file; the response is `{"id": "<file_fs_id>"}`.
pub async fn upload_blks_api(
    State(state): State<Arc<AppState>>,
    Path(token): Path<String>,
    uri: axum::http::Uri,
    mut multipart: Multipart,
) -> Result<axum::response::Response, AppError> {
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

    // Block-level uploads name their own blocks (`filename` = block id, which
    // must equal `sha1(plaintext)` below) and are committed by replaying that
    // same client-supplied id list. Server-side encryption would have to store
    // the block under `sha1(ciphertext)` instead, so the ids the client later
    // commits would not resolve. Upstream only ever used this API for encrypted
    // libraries with *client-side* encryption (a path the current iOS client no
    // longer calls), so encrypted libraries are refused here rather than
    // silently storing plaintext under a mismatched id.
    if let Some(repo_model) = state.repos.repo.find_by_id(&info.repo_id).await?
        && repo_model.encrypted != 0
    {
        return Err(AppError::BadRequest(
            "block upload is not supported for encrypted libraries".into(),
        ));
    }

    let uid = Some(info.user_id);
    let query = query_params(&uri);
    let mut fields: HashMap<String, String> = HashMap::new();
    let mut new_block_ids: Vec<String> = Vec::new();

    while let Some(mut field) = multipart
        .next_field()
        .await
        .map_err(|e| AppError::Internal(format!("multipart error: {e}")))?
    {
        let name = field.name().unwrap_or("").to_string();
        if name == "file" {
            let block_id = field.file_name().unwrap_or("").to_string();
            let data = read_multipart_field_limited(&mut field, MAX_BLOCK_UPLOAD_BYTES).await?;
            if !block_id.is_empty() && !data.is_empty() {
                // Verify the SHA-1 and write immediately, so one request cannot
                // buffer every block in memory at once.
                let computed = infra::crypto::fs_id::sha1_hex(&data);
                if computed != block_id {
                    return Err(AppError::BadRequest(format!(
                        "block ID mismatch: expected {block_id}, computed {computed}"
                    )));
                }
                // Charge the block before writing it, so a loop that only uploads
                // blocks (never a commit) cannot fill the disk while committed
                // usage stays at zero. 443 is what the Qt client and seaf-daemon
                // map to "out of quota".
                crate::service::fs::quota::reserve_block_bytes(
                    &state.repos,
                    info.user_id,
                    &info.repo_id,
                    data.len() as i64,
                    state.config().storage.default_user_quota_bytes,
                )
                .await?;
                let (_, was_new) = state
                    .block_store
                    .write_block_with_id_tracked(&info.repo_id, &block_id, &data)
                    .await
                    .map_err(|e| {
                        AppError::Internal(format!("failed to write block {block_id}: {e}"))
                    })?;
                if was_new {
                    new_block_ids.push(block_id);
                }
            }
        } else {
            let raw = read_multipart_field_limited(&mut field, MAX_SMALL_BODY_BYTES).await?;
            let text = String::from_utf8(raw)
                .map_err(|_| AppError::BadRequest("invalid utf-8 field value".into()))?;
            fields.insert(name, text);
        }
    }

    if fields.contains_key("commitonly") {
        let parent_dir = fields
            .get("parent_dir")
            .map(|s| s.as_str())
            .unwrap_or(&info.parent_dir);
        let file_name = fields
            .get("file_name")
            .ok_or_else(|| AppError::BadRequest("file_name required for commit".into()))?;
        // Align with seafile's should_ignore_file: reject empty names, path
        // separators, NUL, "." / "..", and over-long names. Deliberately not the
        // strict character blacklist, so quotes, brackets etc. remain uploadable.
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

        let block_ids: Vec<String> = serde_json::from_str(blockids_str)
            .map_err(|_| AppError::BadRequest("invalid blockids JSON array".into()))?;

        if block_ids.is_empty() {
            return Err(AppError::BadRequest("blockids cannot be empty".into()));
        }

        // Reject malformed / path-traversal ids outright — a valid block id is
        // exactly 40 hex chars (content-addressed SHA-1).
        if let Some(bad) = block_ids
            .iter()
            .find(|bid| bid.len() != 40 || !bid.bytes().all(|b| b.is_ascii_hexdigit()))
        {
            return Err(AppError::BadRequest(format!("invalid block id: {bad}")));
        }
        // Stat the blocks concurrently; order does not matter for a sum.
        use futures::StreamExt;
        let owner_repo_id = info.repo_id.clone();
        let sizes: Vec<Result<i64, AppError>> = futures::stream::iter(block_ids.iter().cloned())
            .map(|bid| {
                let store = state.block_store.clone();
                let repo_id = owner_repo_id.clone();
                async move {
                    store
                        .block_size(&repo_id, &bid)
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

        // Pre-check quota against the actual block bytes before assembling. On
        // failure, remove blocks newly written by this request so they don't
        // linger as orphans.
        if let Err(e) = crate::service::fs::quota::check_upload_quota(
            &state.repos,
            info.user_id,
            real_size,
            state.config().storage.default_user_quota_bytes,
        )
        .await
        {
            for id in &new_block_ids {
                if let Err(e) = state.block_store.remove_block(&info.repo_id, id).await {
                    tracing::warn!("failed to cleanup orphaned block {id}: {e}");
                }
            }
            return Err(e);
        }

        // Resolve the target directory **before** writing the file object, via
        // the upload-link-aware helper: an `upload_link_id` token is scoped to
        // the link's directory and must not commit outside it. Rejecting an
        // invalid path here also avoids leaving an orphan fs_object behind.
        let relative_path = fields
            .get("relative_path")
            .map(|s| s.as_str())
            .unwrap_or("");
        let target_dir = compute_scoped_target_dir(&info, parent_dir, relative_path)?;

        let file_fs_data = base::common::FsFileData {
            block_ids: block_ids.clone(),
            size: file_size,
            obj_type: 1,
            version: 1,
        };
        let file_fs_id =
            crate::fs::core::store_fs_file_object(state.db.as_ref(), &info.repo_id, &file_fs_data)
                .await?;

        let now = chrono::Utc::now().timestamp();

        let fp = base::sanitize::safe_join_path(&target_dir, file_name)
            .map_err(|e| AppError::BadRequest(format!("invalid path: {e}")))?;

        // Resolve the parent and its ancestor chain to avoid O(d²) re-resolution.
        let (parent_fs_id, ancestor_chain) =
            crate::fs::core::file_ops::FileOps::resolve_fs_id_chain(
                &state.repos,
                &info.repo_id,
                &target_dir,
            )
            .await
            .map_err(|e| AppError::Internal(format!("resolve parent dir failed: {e}")))?;

        // Pre-commit target state from the parent dirents, avoiding a head walk.
        let parent_data =
            crate::fs::core::read_fs_dir_data(&state.repos, &info.repo_id, &parent_fs_id)
                .await
                .map_err(|e| AppError::Internal(format!("read parent dir failed: {e}")))?;
        let existing = parent_data.dirents.iter().find(|d| d.name == *file_name);
        let file_exists = existing.is_some();
        let old_size = existing.map(|d| d.size).unwrap_or(0);

        let entry_name = file_name.clone();
        let modifier_name = info.username.clone();
        // The stored name may differ from the requested one: without `replace`, a
        // collision is resolved with `name (1).ext`, which the caller reports back.
        let mut effective_name = entry_name.clone();
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
                if dirents.iter().any(|d| d.name == entry_name) {
                    let unique_name =
                        infra::common::util::generate_unique_filename(dirents, &entry_name);
                    effective_name = unique_name.clone();
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

        // Adjust repo size and log activity.
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

        return Ok(upload_commit_response(
            wants_ret_json(&query, &fields),
            &file_fs_id,
            &effective_name,
            file_size,
        ));
    }

    Ok(ok_json().into_response())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// OkHttp (used by the Android client) adds Content-Length to each part, so
    /// field-name extraction must not assume Content-Disposition is the only
    /// header: splitting on `"` instead of strip_suffix('"') keeps the name.
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

    /// Build a single-part multipart body carrying `data` as the `file` field.
    fn multipart_body(boundary: &str, data: &[u8]) -> String {
        format!(
            "--{boundary}\r\n\
             Content-Disposition: form-data; name=\"file\"; filename=\"f.bin\"\r\n\
             Content-Type: application/octet-stream\r\n\r\n\
             {}\r\n\
             --{boundary}--\r\n",
            String::from_utf8_lossy(data)
        )
    }

    /// A chunk declaring more than the per-chunk cap is rejected from the header.
    #[tokio::test]
    async fn test_read_chunked_field_rejects_oversized_header() {
        let body = multipart_body("HDRBOUND", b"hi");
        let stream = futures_util::stream::once(async move {
            Result::<bytes::Bytes, std::convert::Infallible>::Ok(bytes::Bytes::from(
                body.into_bytes(),
            ))
        });
        let mut multipart = multer::Multipart::new(stream, "HDRBOUND");
        let mut field = multipart.next_field().await.unwrap().unwrap();

        // Declared chunk is 4096 bytes, cap is 1024 — rejected from the header.
        let err = read_chunked_field(&mut field, Some("bytes 0-4095/4096"), 1024)
            .await
            .unwrap_err();
        assert!(matches!(err, AppError::BadRequest(_)));
    }

    /// A client that lies about the header is cut off at the actual-bytes cap.
    #[tokio::test]
    async fn test_read_chunked_field_rejects_oversized_body() {
        let body = multipart_body("BODYBOUND", b"01234567890123456789");
        let stream = futures_util::stream::once(async move {
            Result::<bytes::Bytes, std::convert::Infallible>::Ok(bytes::Bytes::from(
                body.into_bytes(),
            ))
        });
        let mut multipart = multer::Multipart::new(stream, "BODYBOUND");
        let mut field = multipart.next_field().await.unwrap().unwrap();

        // Header says 10 bytes (within the cap) but the actual body is 20.
        let err = read_chunked_field(&mut field, Some("bytes 0-9/100"), 10)
            .await
            .unwrap_err();
        assert!(matches!(err, AppError::BadRequest(_)));
    }

    #[tokio::test]
    async fn test_read_chunked_field_reads_valid_chunk() {
        let body = multipart_body("OKBOUND", b"hello");
        let stream = futures_util::stream::once(async move {
            Result::<bytes::Bytes, std::convert::Infallible>::Ok(bytes::Bytes::from(
                body.into_bytes(),
            ))
        });
        let mut multipart = multer::Multipart::new(stream, "OKBOUND");
        let mut field = multipart.next_field().await.unwrap().unwrap();

        let data = read_chunked_field(&mut field, Some("bytes 0-4/100"), 1024)
            .await
            .unwrap();
        assert_eq!(data, b"hello");
    }

    /// An unparsable Content-Range is rejected instead of reading unbounded.
    #[tokio::test]
    async fn test_read_chunked_field_without_range_rejects() {
        let body = multipart_body("NORANGE", b"fullbody");
        let stream = futures_util::stream::once(async move {
            Result::<bytes::Bytes, std::convert::Infallible>::Ok(bytes::Bytes::from(
                body.into_bytes(),
            ))
        });
        let mut multipart = multer::Multipart::new(stream, "NORANGE");
        let mut field = multipart.next_field().await.unwrap().unwrap();

        let err = read_chunked_field(&mut field, None, 1024)
            .await
            .unwrap_err();
        assert!(matches!(err, AppError::BadRequest(_)));
    }
}
