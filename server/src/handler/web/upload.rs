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

/// Upload links are add-only: the public page never sends `replace=1`, and an
/// anonymous holder must not be able to overwrite files the link creator already
/// had in the target directory. Session/token uploads that are not links keep
/// the normal replace semantics.
fn scoped_replace(requested: bool, upload_link_id: Option<i32>) -> bool {
    requested && upload_link_id.is_none()
}

/// Target directory and write policy shared by every file of a create request.
struct CreateTarget {
    dir: String,
    replace: bool,
}

/// Resolve a create-style target: `parent_dir` (default: the token's directory,
/// or the library root without one) plus `relative_path`. Link tokens are clamped
/// to their directory and may only add.
fn resolve_create_target(
    token: Option<&crate::service::auth::access_token::AccessToken>,
    parent_dir: Option<&str>,
    relative_path: &str,
    replace_requested: bool,
) -> Result<CreateTarget, AppError> {
    let parent_dir = parent_dir
        .or_else(|| token.map(|info| info.parent_dir.as_str()))
        .unwrap_or("/");
    validate_upload_paths(parent_dir, relative_path)?;
    let dir = match token {
        Some(info) => compute_scoped_target_dir(info, parent_dir, relative_path)?,
        None => compute_target_dir(parent_dir, relative_path)?,
    };
    Ok(CreateTarget {
        dir,
        replace: scoped_replace(
            replace_requested,
            token.and_then(|info| info.upload_link_id),
        ),
    })
}

/// Directory and name of an existing entry named by an absolute path.
struct ExistingTarget {
    dir: String,
    name: String,
}

/// Resolve an update target. `path` is absolute (`target_file`, or the session
/// form's `p`); `relative_path` is applied under the path's directory.
fn resolve_existing_target(
    token: Option<&crate::service::auth::access_token::AccessToken>,
    path: &str,
    relative_path: &str,
) -> Result<ExistingTarget, AppError> {
    if relative_path.starts_with('/') {
        return Err(AppError::BadRequest("Invalid relative path".into()));
    }
    let (raw_parent, name) = path.rsplit_once('/').unwrap_or(("/", path));
    let parent = if raw_parent.is_empty() {
        "/"
    } else {
        raw_parent
    };
    let dir = match token {
        Some(info) => compute_scoped_target_dir(info, parent, relative_path)?,
        None => compute_target_dir(parent, relative_path)?,
    };
    Ok(ExistingTarget {
        dir,
        name: name.to_string(),
    })
}

/// Credentials resolved before the body is read (token) or after it (session).
///
/// The two constructors own the order the clients observe: a token endpoint
/// answers invalid-token before it looks at the body; a session endpoint parses
/// the body first (repo_id lives there) and only then checks CSRF and access.
struct AuthContext {
    /// The minted token; `None` for a session caller.
    token: Option<crate::service::auth::access_token::AccessToken>,
    repo_id: String,
    user_id: i32,
    modifier: String,
    enc_key: Option<(Vec<u8>, Vec<u8>)>,
}

impl AuthContext {
    /// Validate a short-lived upload/update token: token → op → write permission
    /// → library key, in that order.
    async fn from_token(state: &AppState, raw: &str, kind: TokenKind) -> Result<Self, AppError> {
        let token = state.token_manager.validate(raw).ok_or_else(|| {
            AppError::BadRequest(format!("invalid or expired {} token", kind.op()))
        })?;
        if token.op != kind.op() {
            return Err(AppError::BadRequest(format!(
                "token not valid for {}",
                kind.op()
            )));
        }
        crate::domain::permission::check_repo_write_permission(
            state.repos.member.as_ref(),
            &token.repo_id,
            token.user_id,
        )
        .await?;
        // A shareable upload link has no password cache to draw on, so an
        // encrypted library is refused outright; every other caller must have
        // warmed the server-side library key (440 otherwise).
        let enc_key = super::download::upload_block_key(
            state,
            &token.repo_id,
            token.user_id,
            token.upload_link_id.is_some(),
        )
        .await?;
        Ok(Self {
            repo_id: token.repo_id.clone(),
            user_id: token.user_id,
            modifier: token.username.clone(),
            token: Some(token),
            enc_key,
        })
    }

    /// A browser session: CSRF → write permission → library key. The handler
    /// calls this only after its own field checks, so their 400s keep winning.
    async fn from_session(
        state: &AppState,
        user: &WebUser,
        repo_id: &str,
        csrf: Option<&str>,
    ) -> Result<Self, AppError> {
        crate::service::auth::csrf::check_form_csrf(state, &user.session_token, csrf)?;
        crate::domain::permission::check_repo_write_permission(
            state.repos.member.as_ref(),
            repo_id,
            user.user_id,
        )
        .await?;
        let enc_key =
            super::download::upload_block_key(state, repo_id, user.user_id, false).await?;
        Ok(Self {
            repo_id: repo_id.to_string(),
            user_id: user.user_id,
            modifier: user.email.clone(),
            token: None,
            enc_key,
        })
    }

    fn enc_key_ref(&self) -> Option<(&[u8], &[u8])> {
        self.enc_key
            .as_ref()
            .map(|(key, iv)| (key.as_slice(), iv.as_slice()))
    }

    /// The minted token; only token-authenticated endpoints call this.
    fn token(&self) -> &crate::service::auth::access_token::AccessToken {
        self.token.as_ref().expect("token-authenticated endpoint")
    }
}

/// Which token operation an endpoint accepts; the words also appear in its 400s.
#[derive(Clone, Copy)]
enum TokenKind {
    Upload,
    Update,
}

impl TokenKind {
    fn op(self) -> &'static str {
        match self {
            TokenKind::Upload => "upload",
            TokenKind::Update => "update",
        }
    }
}

/// Parse seafile's `replace` argument from the URL query (the `upload-link`
/// endpoint appends `?replace=1`) or a multipart form field, as upstream's
/// `r.FormValue("replace")` does. Absent, `0` or `false` keeps both (the new
/// file becomes `name (1).ext`); `1`/`true` overwrites in place; anything else
/// is rejected, matching `server/fileserver/fileop.go:1220-1231`.
///
/// Shared with `/api2/repos/{repo_id}/file/` and the block commit so all three
/// entry points accept the same spellings instead of silently keeping both.
pub(crate) fn parse_replace_arg(query: Option<&str>, form: Option<&str>) -> Result<bool, AppError> {
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
    if !crate::fs::core::entry_exists(&state.repos, repo_id, &path).await {
        return Err(AppError::NotExists);
    }
    Ok(())
}

/// How an endpoint frames a successful upload/update body.
///
/// The `-aj` endpoints always answer the JSON array (`isAjax=true`); the `-api`
/// endpoints answer the plain-text file id unless `ret-json` was requested — the
/// official Android and desktop clients store that body verbatim, so a JSON array
/// there corrupts their state (`server/fileserver/fileop.go:1859-1884`).
#[derive(Clone, Copy)]
enum ReplyStyle {
    Array,
    PlainOrJson { ret_json: bool },
}

impl ReplyStyle {
    fn answers_json(self) -> bool {
        match self {
            ReplyStyle::Array => true,
            ReplyStyle::PlainOrJson { ret_json } => ret_json,
        }
    }
}

/// One committed file of a (possibly multi-file) upload.
struct CommittedFile {
    id: String,
    name: String,
    size: i64,
}

/// Render a committed upload (one or more files).
///
/// The `-aj` endpoints answer a JSON array. The `-api` endpoints answer one id
/// per file separated by `\t`, or the JSON array with `ret-json` — matching
/// `postMultiFiles` in `server/fileserver/fileop.go:1944-1970`.
fn reply_committed(style: ReplyStyle, files: &[CommittedFile]) -> axum::response::Response {
    use axum::response::IntoResponse;

    if style.answers_json() {
        let items: Vec<serde_json::Value> = files
            .iter()
            .map(|f| json!({ "id": f.id, "name": f.name, "size": f.size }))
            .collect();
        return Json(items).into_response();
    }
    let ids: Vec<&str> = files.iter().map(|f| f.id.as_str()).collect();
    (
        [(
            axum::http::header::CONTENT_TYPE,
            "text/plain; charset=utf-8",
        )],
        ids.join("\t"),
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

/// Render a resumable-upload answer: an intermediate chunk is always
/// `{"success": true}`, a final one follows the endpoint's committed framing.
fn reply_chunk(style: ReplyStyle, result: ChunkResult) -> axum::response::Response {
    use axum::response::IntoResponse;

    match result {
        ChunkResult::Intermediate => Json(json!({ "success": true })).into_response(),
        ChunkResult::Committed { id, name, size } => {
            reply_committed(style, &[CommittedFile { id, name, size }])
        }
    }
}

/// Parse an RFC 3339 `last_modify` / `file_mtime` value the way the Go
/// fileserver does: an unparsable value is silently ignored.
fn parse_mtime(value: Option<&str>) -> Option<i64> {
    let value = value.map(str::trim).filter(|v| !v.is_empty())?;
    chrono::DateTime::parse_from_rfc3339(value)
        .ok()
        .map(|t| t.timestamp())
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

/// Reject a resumable-upload header on an endpoint that does not implement it.
///
/// The upload and update handlers now all support `Content-Range`; the v1
/// `/api2/repos/{repo_id}/file/` endpoint still does not, and ignoring the header
/// there would commit one chunk as the whole file.
pub(crate) fn reject_content_range(headers: &HeaderMap) -> Result<(), AppError> {
    if headers.contains_key(axum::http::header::CONTENT_RANGE) {
        return Err(AppError::BadRequest(
            "Content-Range is not supported on this endpoint".into(),
        ));
    }
    Ok(())
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

/// Remove a finished resumable upload and release its quota reservation.
async fn finish_resumable(
    temp_mgr: &crate::handler::web::temp_file::TempFileManager,
    state: &AppState,
    repo_id: &str,
    file_path: &str,
) {
    if let Some((owner, upload_id)) = temp_mgr.finish(repo_id, file_path).await
        && let Some(uid) = owner
    {
        crate::service::fs::quota::release_upload(
            &state.repos,
            uid,
            repo_id,
            &upload_id.to_string(),
        );
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
    file_data: bytes::Bytes,
    content_range: Option<&str>,
    modifier: &str,
    user_id: Option<i32>,
    enc_key: Option<(&[u8], &[u8])>,
    replace: bool,
    mtime: Option<i64>,
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
    // its final chunk cannot leave quota-exempt blocks behind. The upload id makes
    // a retried first chunk idempotent; the reservation is released on commit,
    // abort, or stale reaping.
    if start == 0 && user_id.is_some() {
        let Some(upload_id) = temp_mgr.upload_id(repo_id, &file_path).await else {
            return Err(AppError::Internal("temp upload disappeared".into()));
        };
        crate::service::fs::quota::reserve_upload_bytes(
            &state.repos,
            repo_id,
            &upload_id.to_string(),
            file_size as i64,
            state.config().storage.default_user_quota_bytes,
        )
        .await?;
    }

    temp_mgr
        .write_chunk(repo_id, &file_path, start, file_data.clone())
        .await
        .map_err(map_temp_error)?;

    // In-transit streaming: in-order chunks feed the CDC chunker so a fully
    // in-order upload commits on the final chunk; out-of-order chunks fall back
    // to temp-file assembly. Encrypted libraries always take the fallback: the
    // stream hashes plaintext (`sha1(blk)`), but they must store `sha1(ciphertext)`.
    if enc_key.is_none() {
        let _ = temp_mgr
            .feed_stream(&state.block_store, repo_id, &file_path, start, &file_data)
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
        let committed = commit_file(
            state,
            CommitRequest {
                repo_id,
                dir: target_dir,
                name: file_name,
                block_ids,
                total_size,
                modifier,
                user_id,
                ensure_dir: true,
                replace,
                new_block_ids,
                mtime,
            },
        )
        .await;
        let committed = match committed {
            Ok(v) => v,
            Err(e) => {
                if matches!(e, AppError::QuotaExceeded) {
                    temp_mgr
                        .abort(&state.repos, repo_id, &file_path, &state.block_store)
                        .await;
                }
                return Err(e);
            }
        };
        finish_resumable(temp_mgr, state, repo_id, &file_path).await;
        return Ok(Some(ChunkResult::Committed {
            id: committed.id,
            name: committed.name,
            size: committed.size,
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

    let committed = commit_file(
        state,
        CommitRequest {
            repo_id,
            dir: target_dir,
            name: file_name,
            block_ids,
            total_size,
            modifier,
            user_id,
            ensure_dir: true,
            replace,
            new_block_ids,
            mtime,
        },
    )
    .await;
    let committed = match committed {
        Ok(v) => v,
        Err(e) => {
            if matches!(e, AppError::QuotaExceeded) {
                temp_mgr
                    .abort(&state.repos, repo_id, &file_path, &state.block_store)
                    .await;
            }
            return Err(e);
        }
    };

    finish_resumable(temp_mgr, state, repo_id, &file_path).await;

    Ok(Some(ChunkResult::Committed {
        id: committed.id,
        name: committed.name,
        size: committed.size,
    }))
}

/// Simple multipart/form-data parser that finds a named field's value and
/// extracts the file (if any) from the body.
///
/// Used instead of axum's `Multipart` extractor, which fails with 400 on the Qt
/// desktop client's uploads.
/// `POST /upload-aj/` — AJAX file upload from the Seahub web UI (no token),
/// taking `file`, `repo_id`, `parent_dir` and optional `relative_path` fields.
pub async fn upload_aj(
    user: WebUser,
    State(state): State<Arc<AppState>>,
    uri: axum::http::Uri,
    headers: HeaderMap,
    body: axum::body::Body,
) -> Result<axum::response::Response, AppError> {
    // No Content-Length precheck here: this endpoint can replace an existing
    // file, and the old size is only known after the multipart fields are parsed.
    // A full-size precheck would reject a legitimate same-size overwrite near the
    // quota; the commit-time delta check is authoritative and failure cleanup
    // removes any blocks this request created.
    //
    // Non-chunked uploads are staged on disk here and only ingested after the
    // session checks below pass; a `Content-Range` chunk is buffered instead.
    let is_chunked = headers.contains_key("content-range");
    let content_range = headers.get("content-range").and_then(|v| v.to_str().ok());
    let mode = if is_chunked {
        IntakeMode::Chunk {
            max_chunk_bytes: state.config().server.max_chunk_size_mb * 1024 * 1024,
        }
    } else {
        IntakeMode::Stage {
            dir: upload_staging_dir(&state),
            max_bytes: max_upload_bytes(&state),
        }
    };
    let Parts {
        fields,
        files: staged_files,
        chunked,
    } = read_parts(&state, mode, body, &headers, usize::MAX).await?;

    let repo_id = fields
        .get("repo_id")
        .ok_or_else(|| AppError::BadRequest("repo_id required".into()))?;
    let relative_path = fields
        .get("relative_path")
        .map(|s| s.as_str())
        .unwrap_or("");
    let query = query_params(&uri);
    // A missing `parent_dir` keeps this endpoint's long-standing default of the
    // library root: the session form only ever came from the web UI, but a body
    // without the field must not start answering 400.
    let target = resolve_create_target(
        None,
        fields.get("parent_dir").map(String::as_str),
        relative_path,
        parse_replace_arg(
            query.get("replace").map(String::as_str),
            fields.get("replace").map(String::as_str),
        )?,
    )?;

    let auth = AuthContext::from_session(
        &state,
        &user,
        repo_id,
        fields.get("csrf_token").map(String::as_str),
    )
    .await?;

    if is_chunked {
        let (filename, file_data) =
            chunked.ok_or_else(|| AppError::BadRequest("No file in multipart form".into()))?;
        if file_data.is_empty() {
            return Err(AppError::BadRequest("No file in multipart form".into()));
        }
        let mtime = parse_mtime(fields.get("file_mtime").map(String::as_str));
        if let Some(result) = try_handle_chunked(
            &state.temp_file_manager,
            &state,
            repo_id,
            &target.dir,
            &filename,
            bytes::Bytes::from(file_data),
            content_range,
            &auth.modifier,
            Some(auth.user_id),
            auth.enc_key_ref(),
            target.replace,
            mtime,
        )
        .await?
        {
            return Ok(reply_chunk(ReplyStyle::Array, result));
        }
        return Err(AppError::Internal(
            "chunked upload produced no outcome".into(),
        ));
    }

    if staged_files.is_empty() {
        return Err(AppError::BadRequest("No file in multipart form".into()));
    }

    // Ingest the staged bytes now that the caller is authorized; each guard
    // removes its staging file on every exit path.
    let mut files = ingest_parts(
        staged_files,
        state.block_store.clone(),
        repo_id,
        auth.enc_key_ref(),
    )
    .await?;
    assign_mtimes(&mut files, &fields);
    if !target.replace && duplicate_file_names(&files) {
        return Err(AppError::BadRequest("Too many files with same name".into()));
    }
    // Zero-byte files are committed too: seafile gives them the `EMPTY_SHA1`
    // sentinel id but still creates the dirent (`server/fileserver/fileop.go:3699-3702`).
    let committed = commit_uploads(
        &state,
        repo_id,
        &target.dir,
        files,
        &auth.modifier,
        Some(auth.user_id),
        true,
        target.replace,
    )
    .await?;
    Ok(reply_committed(ReplyStyle::Array, &committed))
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

/// A file part as it came off the wire.
///
/// Token endpoints authenticate before the body, so their parts are already
/// ingested into blocks; session endpoints carry `repo_id` in the body, so their
/// bytes wait in a staging file until authorization passes.
enum PartFile {
    Ingested(UploadedFile),
    Staged {
        filename: String,
        staged: StagedUpload,
    },
}

impl PartFile {
    /// Turn a part into an ingested file, running the CDC pass for a staged one.
    async fn into_uploaded(
        self,
        store: infra::storage::DynBlockStorage,
        repo_id: &str,
        enc_key: Option<(&[u8], &[u8])>,
    ) -> Result<UploadedFile, AppError> {
        match self {
            PartFile::Ingested(file) => Ok(file),
            PartFile::Staged { filename, staged } => {
                let Some(path) = staged.path() else {
                    return Err(AppError::BadRequest("No file in multipart form".into()));
                };
                let (block_ids, total_size, new_block_ids) =
                    ingest_staged_file(store, repo_id, path.to_path_buf(), enc_key).await?;
                Ok(UploadedFile {
                    filename,
                    block_ids,
                    total_size,
                    new_block_ids,
                    mtime: None,
                })
            }
        }
    }
}

/// Turn every part of a parsed request into an ingested file.
async fn ingest_parts(
    parts: Vec<PartFile>,
    store: infra::storage::DynBlockStorage,
    repo_id: &str,
    enc_key: Option<(&[u8], &[u8])>,
) -> Result<Vec<UploadedFile>, AppError> {
    let mut files = Vec::with_capacity(parts.len());
    for part in parts {
        files.push(part.into_uploaded(store.clone(), repo_id, enc_key).await?);
    }
    Ok(files)
}

/// How a multipart body is taken in.
enum IntakeMode {
    /// Token endpoints: the repository is known before the body, so each file
    /// part streams straight into the block store.
    Stream {
        repo_id: String,
        enc_key: Option<(Vec<u8>, Vec<u8>)>,
    },
    /// Session endpoints: `repo_id` is in the body, so bytes go to a staging file
    /// until authorization passes.
    Stage {
        dir: std::path::PathBuf,
        max_bytes: u64,
    },
    /// `Content-Range`: exactly one chunk, buffered because the resumable path
    /// needs it whole.
    Chunk { max_chunk_bytes: u64 },
}

/// A parsed multipart request.
struct Parts {
    fields: HashMap<String, String>,
    files: Vec<PartFile>,
    /// The single chunk of a `Content-Range` request.
    chunked: Option<(String, Vec<u8>)>,
}

/// Read the multipart body in one of the three intake modes.
///
/// `max_files` is 1 for the update endpoints (a second part answers
/// `More files in one request`) and unbounded for the upload ones; a chunked
/// request is always exactly one file.
async fn read_parts(
    state: &AppState,
    mode: IntakeMode,
    body: axum::body::Body,
    headers: &HeaderMap,
    max_files: usize,
) -> Result<Parts, AppError> {
    let boundary = extract_multipart_boundary(headers)?;
    let mut multipart = multer::Multipart::new(body.into_data_stream(), boundary);
    let content_range = headers.get("content-range").and_then(|v| v.to_str().ok());

    let mut parts = Parts {
        fields: HashMap::new(),
        files: Vec::new(),
        chunked: None,
    };
    while let Some(mut field) = multipart
        .next_field()
        .await
        .map_err(|e| AppError::Internal(format!("multipart error: {e}")))?
    {
        let name = field.name().unwrap_or("").to_string();
        if name != "file" {
            parts.fields.insert(
                name,
                field
                    .text()
                    .await
                    .map_err(|e| AppError::Internal(format!("multipart field error: {e}")))?,
            );
            continue;
        }
        let seen = parts.files.len() + usize::from(parts.chunked.is_some());
        if seen >= max_files {
            return Err(AppError::BadRequest("More files in one request".into()));
        }
        let filename = field.file_name().unwrap_or("unknown").to_string();
        match &mode {
            IntakeMode::Chunk { max_chunk_bytes } => {
                let data = read_chunked_field(&mut field, content_range, *max_chunk_bytes).await?;
                parts.chunked = Some((filename, data));
            }
            IntakeMode::Stage { dir, max_bytes } => {
                let (path, _) = stage_file_field(&mut field, dir, *max_bytes).await?;
                parts.files.push(PartFile::Staged {
                    filename,
                    staged: StagedUpload(Some(path)),
                });
            }
            IntakeMode::Stream { repo_id, enc_key } => {
                let key = enc_key.as_ref().map(|(k, i)| (k.as_slice(), i.as_slice()));
                let (block_ids, total_size, new_block_ids) =
                    stream_file_into_blocks(state.block_store.clone(), repo_id, field, key).await?;
                parts.files.push(PartFile::Ingested(UploadedFile {
                    filename,
                    block_ids,
                    total_size,
                    new_block_ids,
                    mtime: None,
                }));
            }
        }
    }
    Ok(parts)
}

/// A byte stream fed to the CDC chunker.
type ChunkStream<'a> = futures::stream::BoxStream<'a, Result<bytes::Bytes, AppError>>;

/// CDC a byte stream straight into content-defined blocks in the block store, so
/// a file is never fully buffered in memory; `Chunker::new(0)` uses seafile's
/// fixed 8 MiB avg / 6 MiB min / 10 MiB max sizes.
///
/// `enc_key` is the library block key for an encrypted repository (see
/// `upload_block_key`); when set, each block is encrypted before it is stored
/// and its id becomes `sha1(ciphertext)`, the Seafile convention
/// (`common/fs-mgr.c:707-721` hashes the encrypted buffer).
async fn ingest(
    store: infra::storage::DynBlockStorage,
    repo_id: &str,
    enc_key: Option<(&[u8], &[u8])>,
    mut stream: ChunkStream<'_>,
) -> Result<(Vec<String>, i64, Vec<String>), AppError> {
    use futures::StreamExt;

    crate::fs::core::FileOps::stream_blocks_pipelined(
        &store,
        repo_id,
        enc_key,
        move |tx| async move {
            let mut chunker = infra::storage::cdc::Chunker::new(0);
            let mut total_size = 0i64;
            let mut idx = 0usize;
            while let Some(chunk) = stream.next().await {
                let chunk = chunk?;
                for block in chunker.feed(&chunk) {
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

/// The block-streaming half of [`ingest`], for a multipart file part. Still
/// `pub(crate)`: `/api2/repos/{id}/file/` streams its one part the same way.
pub(crate) async fn stream_file_into_blocks(
    store: infra::storage::DynBlockStorage,
    repo_id: &str,
    field: multer::Field<'_>,
    enc_key: Option<(&[u8], &[u8])>,
) -> Result<(Vec<String>, i64, Vec<String>), AppError> {
    use futures::StreamExt;

    let stream: ChunkStream<'_> = Box::pin(
        field.map(|chunk| chunk.map_err(|e| AppError::Internal(format!("file read error: {e}")))),
    );
    ingest(store, repo_id, enc_key, stream).await
}

/// CDC a staged file into the block store (called only after authorization).
async fn ingest_staged_file(
    store: infra::storage::DynBlockStorage,
    repo_id: &str,
    path: std::path::PathBuf,
    enc_key: Option<(&[u8], &[u8])>,
) -> Result<(Vec<String>, i64, Vec<String>), AppError> {
    use futures::StreamExt;

    let file = tokio::fs::File::open(&path)
        .await
        .map_err(|e| AppError::Internal(format!("open staged file: {e}")))?;
    let stream: ChunkStream<'_> = Box::pin(
        tokio_util::io::ReaderStream::new(file)
            .map(|chunk| chunk.map_err(|e| AppError::Internal(format!("read staged file: {e}")))),
    );
    ingest(store, repo_id, enc_key, stream).await
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

/// Staging directory for uploads whose authorization happens after parsing.
fn upload_staging_dir(state: &AppState) -> std::path::PathBuf {
    state.config().storage.temp_dir.join("staging")
}

/// Maximum upload size in bytes (`0` = unlimited).
fn max_upload_bytes(state: &AppState) -> u64 {
    state.config().server.max_upload_size_mb * 1024 * 1024
}

/// One file whose bytes were already turned into blocks (token endpoints).
struct UploadedFile {
    filename: String,
    block_ids: Vec<String>,
    total_size: i64,
    new_block_ids: Vec<String>,
    mtime: Option<i64>,
}

/// Reject `parent_dir`/`relative_path` shapes the Go fileserver rejects:
/// `//` in the parent and a leading `/` on the relative path.
fn validate_upload_paths(parent_dir: &str, relative_path: &str) -> Result<(), AppError> {
    if parent_dir.contains("//") {
        return Err(AppError::BadRequest(
            "parent_dir contains // sequence".into(),
        ));
    }
    if relative_path.starts_with('/') {
        return Err(AppError::BadRequest("Invalid relative path".into()));
    }
    Ok(())
}

/// Apply `last_modify` (single) or `last_modify_list` (one per file) to the
/// parsed files. An unparsable value is ignored, as in the Go fileserver.
fn assign_mtimes(files: &mut [UploadedFile], fields: &HashMap<String, String>) {
    if files.len() == 1 {
        files[0].mtime = parse_mtime(fields.get("last_modify").map(String::as_str));
        return;
    }
    let Some(list) = fields.get("last_modify_list") else {
        return;
    };
    let values: Vec<&str> = list.split(',').map(str::trim).collect();
    if values.len() != files.len() {
        return;
    }
    for (file, value) in files.iter_mut().zip(values) {
        file.mtime = parse_mtime(Some(value));
    }
}

/// Whether two files in one request share a name. Upstream rejects that unless
/// `replace` is set (`checkFilesWithSameName`, `fileop.go:1873`).
fn duplicate_file_names(files: &[UploadedFile]) -> bool {
    let mut seen = std::collections::HashSet::with_capacity(files.len());
    files.iter().any(|f| !seen.insert(f.filename.as_str()))
}

/// One file to commit: the bytes are already ingested, only the commit is left.
struct CommitRequest<'a> {
    repo_id: &'a str,
    dir: &'a str,
    name: &'a str,
    block_ids: Vec<String>,
    total_size: i64,
    modifier: &'a str,
    user_id: Option<i32>,
    ensure_dir: bool,
    replace: bool,
    new_block_ids: Vec<String>,
    mtime: Option<i64>,
}

/// Commit one ingested file through the service that owns replace/unique-name,
/// quota accounting, activity logging and index scheduling.
async fn commit_file(
    state: &AppState,
    request: CommitRequest<'_>,
) -> Result<CommittedFile, AppError> {
    let CommitRequest {
        repo_id,
        dir,
        name,
        block_ids,
        total_size,
        modifier,
        user_id,
        ensure_dir,
        replace,
        new_block_ids,
        mtime,
    } = request;
    let (id, name) = state
        .file_service()
        .upload_file_committed_stream_at(
            repo_id,
            dir,
            name,
            block_ids,
            total_size,
            modifier,
            user_id,
            ensure_dir,
            replace,
            new_block_ids,
            mtime,
        )
        .await?;
    Ok(CommittedFile {
        id,
        name,
        size: total_size,
    })
}

/// Commit one or more already-ingested files, in request order.
async fn commit_uploads(
    state: &AppState,
    repo_id: &str,
    target_dir: &str,
    files: Vec<UploadedFile>,
    modifier: &str,
    user_id: Option<i32>,
    ensure_dir: bool,
    replace: bool,
) -> Result<Vec<CommittedFile>, AppError> {
    let mut committed = Vec::with_capacity(files.len());
    for file in files {
        committed.push(
            commit_file(
                state,
                CommitRequest {
                    repo_id,
                    dir: target_dir,
                    name: &file.filename,
                    block_ids: file.block_ids,
                    total_size: file.total_size,
                    modifier,
                    user_id,
                    ensure_dir,
                    replace,
                    new_block_ids: file.new_block_ids,
                    mtime: file.mtime,
                },
            )
            .await?,
        );
    }
    Ok(committed)
}

/// `POST /update-api/` — update an existing file from the web UI (no token),
/// taking `file`, `repo_id`, and `p` / `path` (the full target path).
pub async fn update_api(
    user: WebUser,
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    body: axum::body::Body,
) -> Result<axum::response::Response, AppError> {
    // An update always replaces an existing file, so the bytes it frees are part
    // of the budget. A Content-Length precheck cannot know that delta and used to
    // reject a same-size replacement near the quota; the commit-time check in
    // `upload_file_committed_stream` applies the correct delta instead.
    let is_chunked = headers.contains_key("content-range");
    let content_range = headers.get("content-range").and_then(|v| v.to_str().ok());
    let mode = if is_chunked {
        IntakeMode::Chunk {
            max_chunk_bytes: state.config().server.max_chunk_size_mb * 1024 * 1024,
        }
    } else {
        IntakeMode::Stage {
            dir: upload_staging_dir(&state),
            max_bytes: max_upload_bytes(&state),
        }
    };
    // An update carries exactly one file part.
    let Parts {
        fields,
        files,
        chunked,
    } = read_parts(&state, mode, body, &headers, 1).await?;

    let repo_id = fields.get("repo_id").cloned().unwrap_or_default();
    let file_path = fields
        .get("p")
        .or_else(|| fields.get("path"))
        .cloned()
        .unwrap_or_default();
    let csrf_token = fields.get("csrf_token").cloned();

    if file_path.is_empty() {
        return Err(AppError::BadRequest("Path is missing.".into()));
    }
    if !file_path.starts_with('/') {
        return Err(AppError::BadRequest("Path is missing or invalid.".into()));
    }

    let auth = AuthContext::from_session(&state, &user, &repo_id, csrf_token.as_deref()).await?;

    let target = resolve_existing_target(None, &file_path, "")?;
    require_target_exists(&state, &auth.repo_id, &target.dir, &target.name).await?;

    if is_chunked {
        let (_, file_data) =
            chunked.ok_or_else(|| AppError::BadRequest("No file in multipart form".into()))?;
        if file_data.is_empty() {
            return Err(AppError::BadRequest("No file in multipart form".into()));
        }
        let mtime = parse_mtime(fields.get("file_mtime").map(String::as_str));
        if let Some(result) = try_handle_chunked(
            &state.temp_file_manager,
            &state,
            &repo_id,
            &target.dir,
            &target.name,
            bytes::Bytes::from(file_data),
            content_range,
            &auth.modifier,
            Some(auth.user_id),
            auth.enc_key_ref(),
            true,
            mtime,
        )
        .await?
        {
            return Ok(reply_chunk(ReplyStyle::Array, result));
        }
        return Err(AppError::Internal(
            "chunked upload produced no outcome".into(),
        ));
    }

    let Some(part) = files.into_iter().next() else {
        return Err(AppError::BadRequest("No file in multipart form".into()));
    };
    let file = part
        .into_uploaded(state.block_store.clone(), &repo_id, auth.enc_key_ref())
        .await?;
    let mtime = parse_mtime(fields.get("last_modify").map(String::as_str));
    // Update semantics: overwrite in place, and commit zero-byte files too.
    let committed = commit_file(
        &state,
        CommitRequest {
            repo_id: &repo_id,
            dir: &target.dir,
            name: &target.name,
            block_ids: file.block_ids,
            total_size: file.total_size,
            modifier: &auth.modifier,
            user_id: Some(auth.user_id),
            ensure_dir: true,
            replace: true,
            new_block_ids: file.new_block_ids,
            mtime,
        },
    )
    .await?;
    Ok(reply_committed(ReplyStyle::Array, &[committed]))
}

/// `POST /update-aj/` — AJAX file update from the Seahub web UI (no token),
/// taking `file`, `repo_id` and `target_file` (the full target path).
pub async fn update_aj(
    user: WebUser,
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    body: axum::body::Body,
) -> Result<axum::response::Response, AppError> {
    // See `update_api`: an update replaces the file, so only the commit-time
    // size delta is a correct quota check.
    let is_chunked = headers.contains_key("content-range");
    let content_range = headers.get("content-range").and_then(|v| v.to_str().ok());
    let mode = if is_chunked {
        IntakeMode::Chunk {
            max_chunk_bytes: state.config().server.max_chunk_size_mb * 1024 * 1024,
        }
    } else {
        IntakeMode::Stage {
            dir: upload_staging_dir(&state),
            max_bytes: max_upload_bytes(&state),
        }
    };
    // An update carries exactly one file part.
    let Parts {
        fields,
        files,
        chunked,
    } = read_parts(&state, mode, body, &headers, 1).await?;

    let repo_id = fields
        .get("repo_id")
        .ok_or_else(|| AppError::BadRequest("repo_id required".into()))?;
    let target_file = fields
        .get("target_file")
        .ok_or_else(|| AppError::BadRequest("target_file required".into()))?;

    let auth = AuthContext::from_session(
        &state,
        &user,
        repo_id,
        fields.get("csrf_token").map(|s| s.as_str()),
    )
    .await?;

    let target = resolve_existing_target(None, target_file, "")?;
    require_target_exists(&state, &auth.repo_id, &target.dir, &target.name).await?;

    if is_chunked {
        let (_, file_data) =
            chunked.ok_or_else(|| AppError::BadRequest("No file in multipart form".into()))?;
        let file_data = bytes::Bytes::from(file_data);
        if file_data.is_empty() {
            return Err(AppError::BadRequest("No file in multipart form".into()));
        }
        if let Some(result) = try_handle_chunked(
            &state.temp_file_manager,
            &state,
            repo_id,
            &target.dir,
            &target.name,
            file_data,
            content_range,
            &auth.modifier,
            Some(auth.user_id),
            auth.enc_key_ref(),
            // The update endpoints always replace the target in place.
            true,
            parse_mtime(fields.get("file_mtime").map(String::as_str)),
        )
        .await?
        {
            return Ok(reply_chunk(ReplyStyle::Array, result));
        }
        return Err(AppError::Internal(
            "chunked upload produced no outcome".into(),
        ));
    }

    let Some(part) = files.into_iter().next() else {
        return Err(AppError::BadRequest("No file in multipart form".into()));
    };
    let file = part
        .into_uploaded(state.block_store.clone(), repo_id, auth.enc_key_ref())
        .await?;
    let mtime = parse_mtime(fields.get("last_modify").map(String::as_str));
    // Update semantics: overwrite in place, and commit zero-byte files too.
    let committed = commit_file(
        &state,
        CommitRequest {
            repo_id,
            dir: &target.dir,
            name: &target.name,
            block_ids: file.block_ids,
            total_size: file.total_size,
            modifier: &auth.modifier,
            user_id: Some(auth.user_id),
            ensure_dir: true,
            replace: true,
            new_block_ids: file.new_block_ids,
            mtime,
        },
    )
    .await?;
    Ok(reply_committed(ReplyStyle::Array, &[committed]))
}

/// `POST /upload-aj/{token}` — upload with an upload-link token, as sent by the
/// Seahub React frontend after `/api2/repos/{id}/upload-link/?from=web`.
pub async fn upload_aj_token(
    State(state): State<Arc<AppState>>,
    Path(token): Path<String>,
    uri: axum::http::Uri,
    headers: HeaderMap,
    body: axum::body::Body,
) -> Result<axum::response::Response, AppError> {
    let auth = AuthContext::from_token(&state, &token, TokenKind::Upload).await?;

    // No Content-Length quota pre-check: a normal (non-link) upload token can
    // replace an existing file, so only the commit-time size delta is correct.
    // This matches `/upload-aj/` and `/upload-api/`; the commit removes the
    // blocks it wrote when the delta does not fit.
    let is_chunked = headers.contains_key("content-range");
    let content_range = headers.get("content-range").and_then(|v| v.to_str().ok());
    let mode = if is_chunked {
        IntakeMode::Chunk {
            max_chunk_bytes: state.config().server.max_chunk_size_mb * 1024 * 1024,
        }
    } else {
        // The token already names the repository, so bytes stream straight to
        // blocks instead of waiting in a staging file.
        IntakeMode::Stream {
            repo_id: auth.repo_id.clone(),
            enc_key: auth.enc_key.clone(),
        }
    };
    let Parts {
        fields,
        files,
        chunked,
    } = read_parts(&state, mode, body, &headers, usize::MAX).await?;

    // The public upload page posts only the `file` part and relies on the minted
    // token for the directory, so a body without `parent_dir` keeps that default
    // instead of answering 400. The token's directory is still clamped, never
    // widened.
    let relative_path = fields
        .get("relative_path")
        .map(|s| s.as_str())
        .unwrap_or("");
    let query = query_params(&uri);
    let target = resolve_create_target(
        Some(auth.token()),
        fields.get("parent_dir").map(String::as_str),
        relative_path,
        parse_replace_arg(
            query.get("replace").map(String::as_str),
            fields.get("replace").map(String::as_str),
        )?,
    )?;

    if is_chunked {
        let (filename, file_data) =
            chunked.ok_or_else(|| AppError::BadRequest("No file in multipart form".into()))?;
        if file_data.is_empty() {
            return Err(AppError::BadRequest("No file in multipart form".into()));
        }
        let mtime = parse_mtime(fields.get("file_mtime").map(String::as_str));
        if let Some(result) = try_handle_chunked(
            &state.temp_file_manager,
            &state,
            &auth.repo_id,
            &target.dir,
            &filename,
            bytes::Bytes::from(file_data),
            content_range,
            &auth.modifier,
            // Upload-link chunked uploads count against the owner's quota.
            Some(auth.user_id),
            auth.enc_key_ref(),
            target.replace,
            mtime,
        )
        .await?
        {
            return Ok(reply_chunk(ReplyStyle::Array, result));
        }
        return Err(AppError::Internal(
            "chunked upload produced no outcome".into(),
        ));
    }

    if files.is_empty() {
        return Err(AppError::BadRequest("No file in multipart form".into()));
    }
    let mut files = ingest_parts(
        files,
        state.block_store.clone(),
        &auth.repo_id,
        auth.enc_key_ref(),
    )
    .await?;
    assign_mtimes(&mut files, &fields);
    if !target.replace && duplicate_file_names(&files) {
        return Err(AppError::BadRequest("Too many files with same name".into()));
    }
    let committed = commit_uploads(
        &state,
        &auth.repo_id,
        &target.dir,
        files,
        &auth.modifier,
        Some(auth.user_id),
        true,
        target.replace,
    )
    .await?;

    if let Some(link_id) = auth.token().upload_link_id {
        upload_link_service::increment_upload_view_cnt(state.repos.clone(), link_id);
    }

    Ok(reply_committed(ReplyStyle::Array, &committed))
}

/// `POST /upload-api/{token}` — token-authenticated upload for the desktop
/// client, which sends a quoted multipart boundary that axum's extractor rejects.
pub async fn upload_api(
    State(state): State<Arc<AppState>>,
    Path(token): Path<String>,
    req: axum::http::Request<axum::body::Body>,
) -> Result<axum::response::Response, AppError> {
    let (parts, body) = req.into_parts();
    let headers = parts.headers;
    // The query has to be captured before the request body is consumed.
    let query = query_params(&parts.uri);
    // Resumable uploads reach this endpoint: the Qt desktop client fetches
    // `api2/repos/{id}/upload-link/` and chunks files over 100 MB
    // (`ReliablePostFileTask`). A chunk must go through the temp-file / assembly
    // path — streaming it straight into blocks would commit it as a whole file,
    // and that path is also where the declared size is reserved against quota.
    let is_chunked = headers.contains_key("content-range");
    let content_range = headers
        .get("content-range")
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned);
    let auth = AuthContext::from_token(&state, &token, TokenKind::Upload).await?;

    // This token can name a replacement, so a full Content-Length precheck would
    // reject a same-size overwrite near the quota. The commit-time delta check is
    // authoritative.
    //
    // Qt's QHttpMultiPart sends a quoted boundary (`boundary="_.Seafile._UUID"`)
    // while the body uses the unquoted form; `extract_multipart_boundary` strips
    // the quotes. The body is consumed as a stream so the file is never buffered.
    let mode = if is_chunked {
        IntakeMode::Chunk {
            max_chunk_bytes: state.config().server.max_chunk_size_mb * 1024 * 1024,
        }
    } else {
        IntakeMode::Stream {
            repo_id: auth.repo_id.clone(),
            enc_key: auth.enc_key.clone(),
        }
    };
    let Parts {
        fields,
        files,
        chunked,
    } = read_parts(&state, mode, body, &headers, usize::MAX).await?;

    // Like the `-aj` token form, a missing `parent_dir` means the directory the
    // link was minted for; the desktop client sends it, older bodies may not.
    let relative_path = fields.get("relative_path").cloned().unwrap_or_default();
    let target = resolve_create_target(
        Some(auth.token()),
        fields.get("parent_dir").map(String::as_str),
        &relative_path,
        parse_replace_arg(
            query.get("replace").map(String::as_str),
            fields.get("replace").map(String::as_str),
        )?,
    )?;
    // Only `/upload-api` honours `ret-json`; official Android/desktop clients
    // read the plain-text body as the file id, so the default stays plain text.
    let ret_json = wants_ret_json(&query, &fields);

    if is_chunked {
        let (filename, file_data) =
            chunked.ok_or_else(|| AppError::BadRequest("No file in multipart form".into()))?;
        if file_data.is_empty() {
            return Err(AppError::BadRequest("No file in multipart form".into()));
        }
        let mtime = parse_mtime(fields.get("file_mtime").map(String::as_str));
        if let Some(result) = try_handle_chunked(
            &state.temp_file_manager,
            &state,
            &auth.repo_id,
            &target.dir,
            &filename,
            bytes::Bytes::from(file_data),
            content_range.as_deref(),
            &auth.modifier,
            Some(auth.user_id),
            auth.enc_key_ref(),
            target.replace,
            mtime,
        )
        .await?
        {
            return Ok(reply_chunk(ReplyStyle::PlainOrJson { ret_json }, result));
        }
        return Err(AppError::Internal(
            "chunked upload produced no outcome".into(),
        ));
    }

    if files.is_empty() {
        return Err(AppError::BadRequest("No file in multipart form".into()));
    }
    let mut files = ingest_parts(
        files,
        state.block_store.clone(),
        &auth.repo_id,
        auth.enc_key_ref(),
    )
    .await?;
    assign_mtimes(&mut files, &fields);
    if !target.replace && duplicate_file_names(&files) {
        return Err(AppError::BadRequest("Too many files with same name".into()));
    }
    let committed = commit_uploads(
        &state,
        &auth.repo_id,
        &target.dir,
        files,
        &auth.modifier,
        Some(auth.user_id),
        true,
        target.replace,
    )
    .await?;
    Ok(reply_committed(
        ReplyStyle::PlainOrJson { ret_json },
        &committed,
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
    let (parts, body) = req.into_parts();
    let headers = parts.headers;
    // The query has to be captured before the request body is consumed.
    let query = query_params(&parts.uri);
    let auth = AuthContext::from_token(&state, &token, TokenKind::Update).await?;

    // See `update_api`: this endpoint only replaces an existing file, so the
    // commit-time delta is the only correct quota check.
    let is_chunked = headers.contains_key("content-range");
    let content_range = headers
        .get("content-range")
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned);

    let mode = if is_chunked {
        IntakeMode::Chunk {
            max_chunk_bytes: state.config().server.max_chunk_size_mb * 1024 * 1024,
        }
    } else {
        IntakeMode::Stream {
            repo_id: auth.repo_id.clone(),
            enc_key: auth.enc_key.clone(),
        }
    };
    // An update carries exactly one file part.
    let Parts {
        fields,
        files,
        chunked,
    } = read_parts(&state, mode, body, &headers, 1).await?;

    let target_file = fields
        .get("target_file")
        .ok_or_else(|| AppError::BadRequest("No target_file given.".into()))?;
    let relative_path = fields.get("relative_path").cloned().unwrap_or_default();
    // `/update-api` honours `ret-json`; without it the body is a plain id.
    let ret_json = wants_ret_json(&query, &fields);

    // Scoped for consistency: an update token never carries `upload_link_id`
    // today, but this keeps the link-directory clamp if one ever does.
    let target = resolve_existing_target(Some(auth.token()), target_file, &relative_path)?;
    require_target_exists(&state, &auth.repo_id, &target.dir, &target.name).await?;

    if is_chunked {
        let (_filename, data) =
            chunked.ok_or_else(|| AppError::BadRequest("No file in multipart form".into()))?;
        if data.is_empty() {
            return Err(AppError::BadRequest("No file in multipart form".into()));
        }
        let mtime = parse_mtime(fields.get("file_mtime").map(String::as_str));
        if let Some(result) = try_handle_chunked(
            &state.temp_file_manager,
            &state,
            &auth.repo_id,
            &target.dir,
            &target.name,
            bytes::Bytes::from(data),
            content_range.as_deref(),
            &auth.modifier,
            Some(auth.user_id),
            auth.enc_key_ref(),
            true,
            mtime,
        )
        .await?
        {
            return Ok(reply_chunk(ReplyStyle::PlainOrJson { ret_json }, result));
        }
        return Err(AppError::Internal(
            "chunked upload produced no outcome".into(),
        ));
    }

    let Some(part) = files.into_iter().next() else {
        return Err(AppError::BadRequest("No file in multipart form".into()));
    };
    let mut file = part
        .into_uploaded(state.block_store.clone(), &auth.repo_id, auth.enc_key_ref())
        .await?;
    file.mtime = parse_mtime(fields.get("last_modify").map(String::as_str));
    let committed = commit_uploads(
        &state,
        &auth.repo_id,
        &target.dir,
        vec![file],
        &auth.modifier,
        Some(auth.user_id),
        false,
        true,
    )
    .await?;
    Ok(reply_committed(
        ReplyStyle::PlainOrJson { ret_json },
        &committed,
    ))
}

/// `POST /update-aj/{token}` — token-based AJAX file update from the Seahub web
/// frontend, taking `file`, `target_file` and optional `relative_path`.
pub async fn update_aj_token(
    State(state): State<Arc<AppState>>,
    Path(token): Path<String>,
    headers: HeaderMap,
    body: axum::body::Body,
) -> Result<axum::response::Response, AppError> {
    let auth = AuthContext::from_token(&state, &token, TokenKind::Update).await?;

    let is_chunked = headers.contains_key("content-range");
    let content_range = headers.get("content-range").and_then(|v| v.to_str().ok());
    let mode = if is_chunked {
        IntakeMode::Chunk {
            max_chunk_bytes: state.config().server.max_chunk_size_mb * 1024 * 1024,
        }
    } else {
        IntakeMode::Stream {
            repo_id: auth.repo_id.clone(),
            enc_key: auth.enc_key.clone(),
        }
    };
    // An update carries exactly one file part.
    let Parts {
        fields,
        files,
        chunked,
    } = read_parts(&state, mode, body, &headers, 1).await?;

    let target_file = fields
        .get("target_file")
        .ok_or_else(|| AppError::BadRequest("No target_file given.".into()))?;
    let relative_path = fields.get("relative_path").cloned().unwrap_or_default();
    // Scoped like every other token upload: an update token never carries
    // `upload_link_id` today, but this keeps the link-directory clamp if one
    // ever does (and matches `update_api_handler`).
    let target = resolve_existing_target(Some(auth.token()), target_file, &relative_path)?;
    require_target_exists(&state, &auth.repo_id, &target.dir, &target.name).await?;

    if is_chunked {
        let (_filename, data) =
            chunked.ok_or_else(|| AppError::BadRequest("No file in multipart form".into()))?;
        if data.is_empty() {
            return Err(AppError::BadRequest("No file in multipart form".into()));
        }
        let mtime = parse_mtime(fields.get("file_mtime").map(String::as_str));
        if let Some(result) = try_handle_chunked(
            &state.temp_file_manager,
            &state,
            &auth.repo_id,
            &target.dir,
            &target.name,
            bytes::Bytes::from(data),
            content_range,
            &auth.modifier,
            Some(auth.user_id),
            auth.enc_key_ref(),
            true,
            mtime,
        )
        .await?
        {
            return Ok(reply_chunk(ReplyStyle::Array, result));
        }
        return Err(AppError::Internal(
            "chunked upload produced no outcome".into(),
        ));
    }

    let Some(part) = files.into_iter().next() else {
        return Err(AppError::BadRequest("No file in multipart form".into()));
    };
    let mut file = part
        .into_uploaded(state.block_store.clone(), &auth.repo_id, auth.enc_key_ref())
        .await?;
    file.mtime = parse_mtime(fields.get("last_modify").map(String::as_str));
    let committed = commit_uploads(
        &state,
        &auth.repo_id,
        &target.dir,
        vec![file],
        &auth.modifier,
        Some(auth.user_id),
        false,
        true,
    )
    .await?;
    Ok(reply_committed(ReplyStyle::Array, &committed))
}

/// `POST /upload-blks-api/{token}` — block upload and commit.
///
/// Without `commitonly`, each `file` part is one block whose filename is its
/// SHA-1, verified before storing. With `commitonly`, the `parent_dir`,
/// `file_name`, `blockids` (JSON array), `file_size` and optional `replace`
/// fields assemble the file; the commit goes through the same service as every
/// other upload endpoint, and the body is the file id as plain text (or a JSON
/// array with `ret-json`).
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

    // A capability URL must stop working when the account is deactivated, like
    // every other token consumer (`/download-api`, `/upload-api`, `/zip`).
    super::download::ensure_token_user_active(&state, info.user_id).await?;

    // Re-check write permission: the token was issued against the caller's
    // membership, but that may have been revoked since (matches download). The
    // owner comes back from the same row so the per-block quota reservation below
    // does not look it up again.
    let owner = crate::domain::permission::repo_write_owner(
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
    let mut has_file = false;

    while let Some(mut field) = multipart
        .next_field()
        .await
        .map_err(|e| AppError::Internal(format!("multipart error: {e}")))?
    {
        let name = field.name().unwrap_or("").to_string();
        if name == "file" {
            has_file = true;
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
                crate::service::fs::quota::reserve_block_bytes_for_owner(
                    &state.repos,
                    owner,
                    &info.repo_id,
                    &block_id,
                    data.len() as i64,
                    state.config().storage.default_user_quota_bytes,
                )
                .await?;
                match state
                    .block_store
                    .write_block_with_id_tracked(&info.repo_id, &block_id, &data)
                    .await
                {
                    Ok((_, true)) => new_block_ids.push(block_id),
                    Ok((_, false)) => {
                        // Already present (retry/concurrent upload): already paid for.
                        crate::service::fs::quota::release_block_bytes(
                            &state.repos,
                            &info.repo_id,
                            &block_id,
                        )
                        .await;
                    }
                    Err(e) => {
                        crate::service::fs::quota::release_block_bytes(
                            &state.repos,
                            &info.repo_id,
                            &block_id,
                        )
                        .await;
                        return Err(AppError::Internal(format!(
                            "failed to write block {block_id}: {e}"
                        )));
                    }
                }
            }
        } else {
            let raw = read_multipart_field_limited(&mut field, MAX_SMALL_BODY_BYTES).await?;
            let text = String::from_utf8(raw)
                .map_err(|_| AppError::BadRequest("invalid utf-8 field value".into()))?;
            fields.insert(name, text);
        }
    }

    let commit_only = fields.contains_key("commitonly") || query.contains_key("commitonly");
    if !commit_only && !has_file {
        return Err(AppError::BadRequest("No file in multipart form".into()));
    }
    // Upstream's update handler requires the commit phase; a block-only request
    // there would otherwise look like an upload and answer success without
    // touching the target.
    if info.op == "update-blks" && !commit_only {
        return Err(AppError::BadRequest(
            "Only commit supported for update-blks-api".into(),
        ));
    }

    if commit_only {
        let blockids_str = fields
            .get("blockids")
            .ok_or_else(|| AppError::BadRequest("blockids required for commit".into()))?;
        let file_size: i64 = fields
            .get("file_size")
            .and_then(|s| s.parse().ok())
            .ok_or_else(|| AppError::BadRequest("file_size required for commit".into()))?;

        let block_ids: Vec<String> = serde_json::from_str(blockids_str)
            .map_err(|_| AppError::BadRequest("invalid blockids JSON array".into()))?;

        // An empty list is the zero-byte file the other upload endpoints accept;
        // a declared size with no blocks is a client error.
        if block_ids.is_empty() && file_size != 0 {
            return Err(AppError::BadRequest(
                "blockids cannot be empty for a non-empty file".into(),
            ));
        }

        // Reject malformed / path-traversal ids outright — a valid block id is
        // exactly 40 lowercase hex chars (content-addressed SHA-1). The sync block
        // PUT shares this predicate, so an id cannot pass one endpoint and fail
        // to resolve in the store behind the other.
        if let Some(bad) = block_ids
            .iter()
            .find(|bid| !crate::handler::is_valid_block_id(bid))
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

        // Resolve the target **before** writing the file object, via the
        // upload-link-aware helper: an `upload_link_id` token is scoped to the
        // link's directory and must not commit outside it. Rejecting an invalid
        // path here also avoids leaving an orphan fs_object behind.
        //
        // An `update-blks` token names the target with `target_file`, like
        // upstream's `update_blks_api_cb`, and always replaces it (441 when the
        // file is gone). An `upload-blks` token keeps the upload contract:
        // `parent_dir` + `relative_path` + `file_name`, optional `replace`. It
        // skips the regular upload path checks (`//` in `parent_dir`), matching
        // the Go fileserver's block commit.
        let (target_dir, file_name, replace, ensure_dir) = if info.op == "update-blks" {
            let target_file = fields
                .get("target_file")
                .ok_or_else(|| AppError::BadRequest("target_file required for update".into()))?;
            let name = target_file
                .rsplit_once('/')
                .map(|(_, n)| n)
                .unwrap_or(target_file);
            if !is_valid_upload_filename(name) {
                return Err(AppError::BadRequest("invalid target_file".into()));
            }
            let target = resolve_existing_target(Some(&info), target_file, "")?;
            require_target_exists(&state, &info.repo_id, &target.dir, &target.name).await?;
            (target.dir, target.name, true, false)
        } else {
            let parent_dir = fields
                .get("parent_dir")
                .map(|s| s.as_str())
                .unwrap_or(&info.parent_dir);
            let file_name = fields
                .get("file_name")
                .ok_or_else(|| AppError::BadRequest("file_name required for commit".into()))?;
            // Align with seafile's should_ignore_file: reject empty names, path
            // separators, NUL, "." / "..", and over-long names. Deliberately not
            // the strict character blacklist, so quotes, brackets etc. remain
            // uploadable.
            if !is_valid_upload_filename(file_name) {
                return Err(AppError::BadRequest("invalid file_name".into()));
            }
            let relative_path = fields
                .get("relative_path")
                .map(|s| s.as_str())
                .unwrap_or("");
            // Link tokens cannot reach this operation today, but keep the add-only
            // rule here too so a future token type cannot inherit overwrite.
            let replace = scoped_replace(
                parse_replace_arg(
                    query.get("replace").map(String::as_str),
                    fields.get("replace").map(String::as_str),
                )?,
                info.upload_link_id,
            );
            (
                compute_scoped_target_dir(&info, parent_dir, relative_path)?,
                file_name.to_string(),
                replace,
                true,
            )
        };

        // Commit through the same service as every other upload entry point, so
        // the directory-replace guard, filename validation, unique naming, quota
        // accounting, activity log and index scheduling are the shared ones. The
        // blocks were written and quota-reserved by the upload loop above; they
        // are released from the reservation table once the commit references them.
        let mtime = parse_mtime(fields.get("last_modify").map(String::as_str));
        let committed = commit_file(
            &state,
            CommitRequest {
                repo_id: &info.repo_id,
                dir: &target_dir,
                name: &file_name,
                block_ids: block_ids.clone(),
                total_size: file_size,
                modifier: &info.username,
                user_id: uid,
                // Upload tokens create missing parents named by `relative_path`;
                // an update token targets an existing file, so its parent exists.
                ensure_dir,
                replace,
                new_block_ids: new_block_ids.clone(),
                mtime,
            },
        )
        .await;

        let committed = match committed {
            Ok(v) => v,
            Err(e) => {
                if matches!(e, AppError::QuotaExceeded) {
                    // The service removed the blocks this request created; drop
                    // their pre-charge as well.
                    for id in &new_block_ids {
                        crate::service::fs::quota::release_block_bytes(
                            &state.repos,
                            &info.repo_id,
                            id,
                        )
                        .await;
                    }
                }
                return Err(e);
            }
        };

        // The blocks are referenced by HEAD now; their uncommitted reservations end.
        crate::service::fs::quota::release_blocks(&state.repos, &info.repo_id, &block_ids).await;

        return Ok(reply_committed(
            ReplyStyle::PlainOrJson {
                ret_json: wants_ret_json(&query, &fields),
            },
            &[committed],
        ));
    }

    Ok(ok_json().into_response())
}
