//! Upload endpoints: multipart intake, CDC ingest, commit and reply framing.
//!
//! The pipeline is `read_parts` (streaming, staged, chunked or block intake)
//! 鈫?target resolution (`resolve_create_target` / `resolve_existing_target`)
//! 鈫?`AuthContext` (the token or session prelude) 鈫?`finish_request` (chunk
//! assembly or whole-file commit, then the endpoint's reply shape).

use axum::{
    Json,
    extract::{Path, State},
    http::HeaderMap,
    response::IntoResponse,
};
use serde_json::json;
use std::collections::HashMap;
use std::sync::Arc;

use crate::AppState;
use crate::handler::{MAX_BLOCK_UPLOAD_BYTES, MAX_SMALL_BODY_BYTES, ok_json};
use crate::service::sharing::link as upload_link_service;
use crate::ui::auth_extractor::WebUser;
use base::error::AppError;

/// Delete blocks an upload request wrote but never committed, and drop any quota
/// reservation the request took for them.
///
/// `cleanup_uncommitted_blocks` re-checks whether a live file references each id,
/// so a block committed by a concurrent identical upload survives. Blocks are
/// content-addressed, so deleting an id that a *concurrent* upload wrote but has
/// not committed yet can only make that upload retry (446) — never corrupt data.
pub(crate) async fn cleanup_block_ids(
    state: &AppState,
    repo_id: &str,
    ids: &[String],
    release_reservations: bool,
) {
    if ids.is_empty() {
        return;
    }
    if release_reservations {
        crate::service::fs::quota::release_blocks(&state.repos, repo_id, ids).await;
    }
    state
        .file_service()
        .cleanup_uncommitted_blocks(repo_id, ids)
        .await;
}

/// Request-scoped cleanup for blocks an upload wrote before its target was
/// validated or committed.
///
/// The upload pipeline streams file parts into the block store before the target
/// path/name is known (the token endpoints receive both in the same multipart
/// body, and a client may send the file part first), and a failure anywhere after
/// that used to leave the blocks unreferenced until the pending-block TTL — 48
/// hours by default. An anonymous upload-link holder could therefore fill the
/// data disk with requests that never commit.
///
/// Every upload handler creates this guard right after parsing the body and
/// disarms it only after a successful commit. Dropping it while still armed
/// schedules the cleanup; `tokio::spawn` is deliberate, because `Drop` cannot
/// await. If the runtime is shutting down the spawn may not run, and the durable
/// pending-block ledger is the backstop it was designed to be.
pub(crate) struct UncommittedBlocks {
    state: Arc<AppState>,
    repo_id: String,
    ids: Vec<String>,
    /// Block-API writes reserve quota per block; streamed uploads do not, so the
    /// guard must not release a reservation it never took (the reservation table
    /// is keyed by block, and a mismatched release would free another upload's).
    release_reservations: bool,
    armed: bool,
}

impl UncommittedBlocks {
    pub(crate) fn new(
        state: Arc<AppState>,
        repo_id: impl Into<String>,
        ids: Vec<String>,
        release_reservations: bool,
    ) -> Self {
        Self {
            state,
            repo_id: repo_id.into(),
            ids,
            release_reservations,
            armed: true,
        }
    }

    /// Add blocks written by a later stage of the same request.
    fn extend(&mut self, ids: impl IntoIterator<Item = String>) {
        self.ids.extend(ids);
    }

    /// The request committed; its blocks are referenced by HEAD now.
    pub(crate) fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for UncommittedBlocks {
    fn drop(&mut self) {
        if !self.armed || self.ids.is_empty() {
            return;
        }
        let state = self.state.clone();
        let repo_id = std::mem::take(&mut self.repo_id);
        let ids = std::mem::take(&mut self.ids);
        let release_reservations = self.release_reservations;
        tokio::spawn(async move {
            cleanup_block_ids(&state, &repo_id, &ids, release_reservations).await;
        });
    }
}

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
    let (parent, name) = infra::common::util::parent_and_name(path);
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
    /// Validate a short-lived upload/update token: token 鈫?op 鈫?write permission
    /// 鈫?library key, in that order.
    async fn from_token(state: &AppState, raw: &str, kind: TokenKind) -> Result<Self, AppError> {
        let ops = [kind.op()];
        let token = crate::service::auth::capability::validate_capability(
            state,
            raw,
            crate::service::auth::capability::TokenPolicy {
                ops: &ops,
                // The family's 400 wording is wire-visible (clients parse the
                // message), so it stays per-kind instead of generic.
                invalid_message: match kind {
                    TokenKind::Upload => "invalid or expired upload token",
                    TokenKind::Update => "invalid or expired update token",
                },
                wrong_op_message: match kind {
                    TokenKind::Upload => "token not valid for upload",
                    TokenKind::Update => "token not valid for update",
                },
                check_upload_link_expiry: true,
            },
        )
        .await?;
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

    /// A browser session: CSRF 鈫?write permission 鈫?library key. The handler
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

    /// Scope that isolates this caller's resumable-upload temp files.
    ///
    /// An upload-link token is its own scope: the token id alone would let two
    /// visitors of the same link (or a re-minted token) collide on one temp file,
    /// so the raw token is folded in through a short SHA-1 prefix. Every other
    /// credential (regular token or browser session) collapses to its user, which
    /// is what makes a resumed upload find its own earlier chunks.
    pub(crate) fn temp_scope(&self) -> String {
        if let Some(token) = &self.token
            && let Some(link_id) = token.upload_link_id
        {
            return format!(
                "link:{link_id}:{}",
                &infra::crypto::fs_id::sha1_hex(token.token.as_bytes())[..16]
            );
        }
        format!("user:{}", self.user_id)
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

/// Collect a URL's query parameters into a map (missing/unparsable 鈫?empty).
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
/// endpoints answer the plain-text file id unless `ret-json` was requested 鈥?the
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
/// per file separated by `\t`, or the JSON array with `ret-json` 鈥?matching
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
    scope: &str,
    repo_id: &str,
    file_path: &str,
) {
    if let Some((owner, upload_id)) = temp_mgr.finish(scope, repo_id, file_path).await
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

/// One `Content-Range` chunk, addressed at a target the handler already resolved.
struct ChunkUpload<'a> {
    dir: &'a str,
    name: &'a str,
    data: bytes::Bytes,
    range: Option<&'a str>,
    replace: bool,
    mtime: Option<i64>,
}

/// Handle a chunked (resumable) upload when a `Content-Range` header is present.
///
/// `Ok(None)` means not chunked (the caller handles it as a regular upload);
/// `Intermediate` means a chunk was stored; `Committed` means the final chunk was
/// assembled and committed, and the caller frames the body for its endpoint.
async fn try_handle_chunked(
    state: &AppState,
    auth: &AuthContext,
    upload: ChunkUpload<'_>,
) -> Result<Option<ChunkResult>, AppError> {
    let ChunkUpload {
        dir: target_dir,
        name: file_name,
        data: file_data,
        range,
        replace,
        mtime,
    } = upload;
    let Some(range_header) = range else {
        return Ok(None); // not a chunked upload
    };
    let temp_mgr = &state.temp_file_manager;
    let repo_id = auth.repo_id.as_str();
    // Every temp-file operation of this request uses the same credential scope,
    // so two callers targeting the same repo/path never touch each other's upload.
    let scope = auth.temp_scope();
    let modifier = auth.modifier.as_str();
    let user_id = Some(auth.user_id);
    let enc_key = auth.enc_key_ref();

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
        .get_or_create(&scope, repo_id, &file_path, file_size)
        .await
        .map_err(map_temp_error)?;
    if let Some(uid) = user_id {
        temp_mgr.bind_owner(&scope, repo_id, &file_path, uid).await;
    }

    // Reserve (not just compare) the declared size so an upload abandoned before
    // its final chunk cannot leave quota-exempt blocks behind. The upload id makes
    // a retried first chunk idempotent; the reservation is released on commit,
    // abort, or stale reaping.
    if start == 0 && user_id.is_some() {
        let Some(upload_id) = temp_mgr.upload_id(&scope, repo_id, &file_path).await else {
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
        .write_chunk(&scope, repo_id, &file_path, start, file_data.clone())
        .await
        .map_err(map_temp_error)?;

    // In-transit streaming: in-order chunks feed the CDC chunker so a fully
    // in-order upload commits on the final chunk; out-of-order chunks fall back
    // to temp-file assembly. Encrypted libraries always take the fallback: the
    // stream hashes plaintext (`sha1(blk)`), but they must store `sha1(ciphertext)`.
    if enc_key.is_none() {
        let _ = temp_mgr
            .feed_stream(
                &state.block_store,
                &scope,
                repo_id,
                &file_path,
                start,
                &file_data,
            )
            .await;
    }

    if end != file_size - 1 {
        return Ok(Some(ChunkResult::Intermediate));
    }

    // Fast path: a fully in-order upload already has its block ids persisted,
    // so commit directly and skip the temp-file re-read.
    if let Some((block_ids, total_size, new_block_ids)) = temp_mgr
        .take_streamed_blocks(&state.block_store, &scope, repo_id, &file_path, file_size)
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
                // Any failed commit leaves the temp upload's blocks uncommitted;
                // abort removes the temp file, deletes the blocks this upload wrote
                // (referenced ones are kept) and releases its reservation. Doing
                // this only for quota failures let every other error hold the disk
                // until the stale-upload reaper.
                temp_mgr
                    .abort(
                        &state.repos,
                        &scope,
                        repo_id,
                        &file_path,
                        &state.block_store,
                    )
                    .await;
                return Err(e);
            }
        };
        finish_resumable(temp_mgr, state, &scope, repo_id, &file_path).await;
        return Ok(Some(ChunkResult::Committed {
            id: committed.id,
            name: committed.name,
            size: committed.size,
        }));
    }

    // Fallback path: stream the temp file through the CDC chunker so the whole
    // file is never buffered in memory, then commit the resulting block ids.
    let Some(stream) = temp_mgr.read_stream(&scope, repo_id, &file_path).await else {
        temp_mgr
            .abort(
                &state.repos,
                &scope,
                repo_id,
                &file_path,
                &state.block_store,
            )
            .await;
        return Err(AppError::Internal(
            "failed to open assembled temp file".into(),
        ));
    };

    // The out-parameter reports blocks written even when the stream fails, so a
    // failed assembly can hand them to `abort` instead of leaving them to GC.
    let mut new_block_ids = Vec::new();
    let streamed = crate::fs::core::FileOps::write_stream_blocks(
        &state.block_store,
        repo_id,
        file_size as usize,
        stream,
        enc_key,
        &mut new_block_ids,
    )
    .await;
    let (block_ids, total_size) = match streamed {
        Ok(v) => v,
        Err(e) => {
            temp_mgr
                .record_new_blocks(&scope, repo_id, &file_path, &new_block_ids)
                .await;
            temp_mgr
                .abort(
                    &state.repos,
                    &scope,
                    repo_id,
                    &file_path,
                    &state.block_store,
                )
                .await;
            return Err(e);
        }
    };

    // Track written blocks so an abort or stale cleanup can delete them.
    temp_mgr
        .record_new_blocks(&scope, repo_id, &file_path, &new_block_ids)
        .await;

    // The streamed total must match the declared file size.
    if total_size as u64 != file_size {
        temp_mgr
            .abort(
                &state.repos,
                &scope,
                repo_id,
                &file_path,
                &state.block_store,
            )
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
            // As in the fast path: every failed commit aborts the temp upload.
            temp_mgr
                .abort(
                    &state.repos,
                    &scope,
                    repo_id,
                    &file_path,
                    &state.block_store,
                )
                .await;
            return Err(e);
        }
    };

    finish_resumable(temp_mgr, state, &scope, repo_id, &file_path).await;

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
/// `POST /upload-aj/` 鈥?AJAX file upload from the Seahub web UI (no token),
/// taking `file`, `repo_id`, `parent_dir` and optional `relative_path` fields.
pub async fn upload_aj(
    user: WebUser,
    State(state): State<Arc<AppState>>,
    uri: axum::http::Uri,
    headers: HeaderMap,
    body: axum::body::Body,
) -> Result<axum::response::Response, AppError> {
    // No Content-Length precheck: the old size is only known after parsing, so
    // only the commit-time size delta is a correct quota check.
    let range = headers.get("content-range").and_then(|v| v.to_str().ok());
    let parts = read_upload_body(&state, None, body, &headers).await?;
    let repo_id = parts
        .fields
        .get("repo_id")
        .cloned()
        .ok_or_else(|| AppError::BadRequest("repo_id required".into()))?;
    let query = query_params(&uri);
    let target = create_target(None, &parts.fields, &query)?;
    let auth = AuthContext::from_session(
        &state,
        &user,
        &repo_id,
        parts.fields.get("csrf_token").map(String::as_str),
    )
    .await?;
    // Everything parsed so far is staged, not ingested, but a chunked request
    // can already have written blocks; the guard is the request's cleanup list.
    let mut guard = UncommittedBlocks::new(
        state.clone(),
        auth.repo_id.clone(),
        parts.written_block_ids(),
        false,
    );
    let finished = finish_create(
        &state,
        &auth,
        parts,
        &target,
        range,
        ReplyStyle::Array,
        &mut guard,
    )
    .await?;
    guard.disarm();
    Ok(finished.response)
}

/// How the final commit names files: uploads take names from the parts, updates
/// commit the single part under the resolved target name.
enum CommitTarget<'a> {
    Create,
    Existing(&'a str),
}

/// Everything after target resolution, shared by the upload and update families.
struct FinishRequest<'a> {
    dir: &'a str,
    target: CommitTarget<'a>,
    replace: bool,
    /// `Content-Range`, when the request is a chunk.
    range: Option<&'a str>,
    style: ReplyStyle,
}

/// The framed reply plus how it was produced, for the one caller that needs to
/// tell a chunked commit from a whole-file one (the upload-link view counter).
struct Finished {
    response: axum::response::Response,
    chunked: bool,
}

/// Run the shared tail: chunk handling, CDC ingest, commit and reply framing.
///
/// `guard` owns the blocks this request wrote before the local validation; the
/// caller disarms it only when a commit succeeded.
async fn finish_request(
    state: &AppState,
    auth: &AuthContext,
    parts: Parts,
    req: FinishRequest<'_>,
    guard: &mut UncommittedBlocks,
) -> Result<Finished, AppError> {
    let FinishRequest {
        dir,
        target,
        replace,
        range,
        style,
    } = req;

    if let Some((filename, data)) = parts.chunked {
        if data.is_empty() {
            return Err(AppError::BadRequest("No file in multipart form".into()));
        }
        let name = match target {
            CommitTarget::Create => filename,
            CommitTarget::Existing(name) => name.to_string(),
        };
        let mtime = parse_mtime(parts.fields.get("file_mtime").map(String::as_str));
        if let Some(result) = try_handle_chunked(
            state,
            auth,
            ChunkUpload {
                dir,
                name: &name,
                data: bytes::Bytes::from(data),
                range,
                replace,
                mtime,
            },
        )
        .await?
        {
            return Ok(Finished {
                response: reply_chunk(style, result),
                chunked: true,
            });
        }
        return Err(AppError::Internal(
            "chunked upload produced no outcome".into(),
        ));
    }

    if parts.files.is_empty() {
        return Err(AppError::BadRequest("No file in multipart form".into()));
    }
    let mut files =
        ingest_parts(state, &auth.repo_id, parts.files, auth.enc_key_ref(), guard).await?;

    match target {
        CommitTarget::Existing(name) => {
            let mut file = files
                .into_iter()
                .next()
                .ok_or_else(|| AppError::BadRequest("No file in multipart form".into()))?;
            file.mtime = parse_mtime(parts.fields.get("last_modify").map(String::as_str));
            let committed = commit_file(
                state,
                CommitRequest {
                    repo_id: &auth.repo_id,
                    dir,
                    name,
                    block_ids: file.block_ids,
                    total_size: file.total_size,
                    modifier: &auth.modifier,
                    user_id: Some(auth.user_id),
                    ensure_dir: false,
                    replace: true,
                    new_block_ids: file.new_block_ids,
                    mtime: file.mtime,
                },
            )
            .await?;
            Ok(Finished {
                response: reply_committed(style, &[committed]),
                chunked: false,
            })
        }
        CommitTarget::Create => {
            assign_mtimes(&mut files, &parts.fields);
            if !replace && duplicate_file_names(&files) {
                return Err(AppError::BadRequest("Too many files with same name".into()));
            }
            let committed = commit_uploads(
                state,
                &auth.repo_id,
                dir,
                files,
                &auth.modifier,
                Some(auth.user_id),
                true,
                replace,
            )
            .await?;
            Ok(Finished {
                response: reply_committed(style, &committed),
                chunked: false,
            })
        }
    }
}

/// Resolve the create target of an upload from its fields and query.
///
/// A missing `parent_dir` means the token's directory for token uploads and the
/// library root for session uploads. Link tokens stay clamped to the directory
/// the link was minted for; nothing here can widen them.
fn create_target(
    token: Option<&crate::service::auth::access_token::AccessToken>,
    fields: &HashMap<String, String>,
    query: &HashMap<String, String>,
) -> Result<CreateTarget, AppError> {
    let relative_path = fields
        .get("relative_path")
        .map(String::as_str)
        .unwrap_or("");
    resolve_create_target(
        token,
        fields.get("parent_dir").map(String::as_str),
        relative_path,
        parse_replace_arg(
            query.get("replace").map(String::as_str),
            fields.get("replace").map(String::as_str),
        )?,
    )
}

/// Finish an upload: names come from the parts, `replace` from the target.
async fn finish_create(
    state: &AppState,
    auth: &AuthContext,
    parts: Parts,
    target: &CreateTarget,
    range: Option<&str>,
    style: ReplyStyle,
    guard: &mut UncommittedBlocks,
) -> Result<Finished, AppError> {
    finish_request(
        state,
        auth,
        parts,
        FinishRequest {
            dir: &target.dir,
            target: CommitTarget::Create,
            replace: target.replace,
            range,
            style,
        },
        guard,
    )
    .await
}

/// Finish an update: the single part is committed under the target's name.
async fn finish_existing(
    state: &AppState,
    auth: &AuthContext,
    parts: Parts,
    target: &ExistingTarget,
    range: Option<&str>,
    style: ReplyStyle,
    guard: &mut UncommittedBlocks,
) -> Result<Finished, AppError> {
    finish_request(
        state,
        auth,
        parts,
        FinishRequest {
            dir: &target.dir,
            target: CommitTarget::Existing(&target.name),
            replace: true,
            range,
            style,
        },
        guard,
    )
    .await
}

/// Intake mode for the token endpoints, where the repository is already known.
fn token_mode(state: &AppState, chunked: bool, auth: &AuthContext) -> IntakeMode {
    if chunked {
        IntakeMode::Chunk {
            max_chunk_bytes: state.config().server.max_chunk_size_mb * 1024 * 1024,
        }
    } else {
        IntakeMode::Stream {
            repo_id: auth.repo_id.clone(),
            enc_key: auth.enc_key.clone(),
            default_name: "unknown",
        }
    }
}

/// Intake mode for the session endpoints, which learn the repository from the body.
fn session_mode(state: &AppState, chunked: bool) -> IntakeMode {
    if chunked {
        IntakeMode::Chunk {
            max_chunk_bytes: state.config().server.max_chunk_size_mb * 1024 * 1024,
        }
    } else {
        IntakeMode::Stage {
            dir: upload_staging_dir(state),
            max_bytes: max_upload_bytes(state),
        }
    }
}

/// Read an upload's parts: many files, streamed for a token caller and staged
/// for a session caller.
async fn read_upload_body(
    state: &AppState,
    auth: Option<&AuthContext>,
    body: axum::body::Body,
    headers: &HeaderMap,
) -> Result<Parts, AppError> {
    let chunked = headers.contains_key("content-range");
    let mode = match auth {
        Some(auth) => token_mode(state, chunked, auth),
        None => session_mode(state, chunked),
    };
    read_parts(state, mode, body, headers, usize::MAX).await
}

/// Read an update's parts: exactly one file.
async fn read_update_body(
    state: &AppState,
    auth: Option<&AuthContext>,
    body: axum::body::Body,
    headers: &HeaderMap,
) -> Result<Parts, AppError> {
    let chunked = headers.contains_key("content-range");
    let mode = match auth {
        Some(auth) => token_mode(state, chunked, auth),
        None => session_mode(state, chunked),
    };
    read_parts(state, mode, body, headers, 1).await
}

/// Extract the multipart `boundary` from the Content-Type header.
///
/// Qt's QHttpMultiPart sends a quoted boundary (`boundary="_.Seafile._UUID"`)
/// while the body uses the unquoted form, so surrounding quotes are stripped.
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
pub(crate) enum PartFile {
    Ingested(UploadedFile),
    Staged {
        filename: String,
        staged: StagedUpload,
    },
}

impl PartFile {
    /// The ingested file of a streaming intake (the only mode the v1 endpoint uses).
    pub(crate) fn into_ingested(self) -> Option<UploadedFile> {
        match self {
            PartFile::Ingested(file) => Some(file),
            PartFile::Staged { .. } => None,
        }
    }

    /// Turn a part into an ingested file, running the CDC pass for a staged one.
    ///
    /// `written` collects the blocks this pass created even when it fails, so the
    /// caller's request guard can delete them.
    async fn into_uploaded(
        self,
        store: infra::storage::DynBlockStorage,
        repo_id: &str,
        enc_key: Option<(&[u8], &[u8])>,
        written: &mut Vec<String>,
    ) -> Result<UploadedFile, AppError> {
        match self {
            PartFile::Ingested(file) => Ok(file),
            PartFile::Staged { filename, staged } => {
                let Some(path) = staged.path() else {
                    return Err(AppError::BadRequest("No file in multipart form".into()));
                };
                let mut new_block_ids = Vec::new();
                let streamed = ingest_staged_file(
                    store,
                    repo_id,
                    path.to_path_buf(),
                    enc_key,
                    &mut new_block_ids,
                )
                .await;
                written.extend(new_block_ids.iter().cloned());
                let (block_ids, total_size) = streamed?;
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
///
/// A failure halfway through adds the already-written staged blocks to the
/// request guard before returning, so they are cleaned up like any other.
async fn ingest_parts(
    state: &AppState,
    repo_id: &str,
    parts: Vec<PartFile>,
    enc_key: Option<(&[u8], &[u8])>,
    guard: &mut UncommittedBlocks,
) -> Result<Vec<UploadedFile>, AppError> {
    let store = state.block_store.clone();
    let mut files = Vec::with_capacity(parts.len());
    for part in parts {
        let mut written = Vec::new();
        let ingested = part
            .into_uploaded(store.clone(), repo_id, enc_key, &mut written)
            .await;
        guard.extend(written);
        files.push(ingested?);
    }
    Ok(files)
}

/// How a multipart body is taken in.
pub(crate) enum IntakeMode {
    /// Token endpoints: the repository is known before the body, so each file
    /// part streams straight into the block store. `default_name` is what a part
    /// without a filename is committed as (empty on the v1 endpoint).
    Stream {
        repo_id: String,
        enc_key: Option<(Vec<u8>, Vec<u8>)>,
        default_name: &'static str,
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
    /// Block upload: every file part is one block whose filename is its SHA-1.
    Blocks { repo_id: String, owner: i32 },
}

/// A parsed multipart request.
pub(crate) struct Parts {
    pub(crate) fields: HashMap<String, String>,
    pub(crate) files: Vec<PartFile>,
    /// The single chunk of a `Content-Range` request.
    chunked: Option<(String, Vec<u8>)>,
    /// Blocks written by a `Blocks` intake, for the quota release on a failed
    /// commit.
    blocks: Vec<String>,
    /// Whether a `Blocks` request carried a file part at all.
    has_file: bool,
}

impl Parts {
    /// Every block this request has already written into the store (streamed
    /// file parts and block-API parts). The chunks and staged modes write nothing
    /// at parse time; staged parts are ingested later, after authorization.
    fn written_block_ids(&self) -> Vec<String> {
        self.files
            .iter()
            .filter_map(|part| match part {
                PartFile::Ingested(file) => Some(file.new_block_ids.iter().cloned()),
                PartFile::Staged { .. } => None,
            })
            .flatten()
            .chain(self.blocks.iter().cloned())
            .collect()
    }
}

/// Read the multipart body in one of the four intake modes.
///
/// `max_files` is 1 for the update endpoints (a second part answers
/// `More files in one request`) and unbounded for the upload ones; a chunked
/// request is always exactly one file.
pub(crate) async fn read_parts(
    state: &AppState,
    mode: IntakeMode,
    body: axum::body::Body,
    headers: &HeaderMap,
    max_files: usize,
) -> Result<Parts, AppError> {
    // Created blocks are tracked here so a body that dies halfway (client
    // disconnect, invalid field, oversized part) does not leave them to the
    // pending-block TTL. `Blocks` reservations are released too; the streamed
    // modes never reserve per block.
    let repo_id = match &mode {
        IntakeMode::Stream { repo_id, .. } | IntakeMode::Blocks { repo_id, .. } => repo_id.clone(),
        IntakeMode::Stage { .. } | IntakeMode::Chunk { .. } => String::new(),
    };
    let release_reservations = matches!(&mode, IntakeMode::Blocks { .. });
    let mut written: Vec<String> = Vec::new();

    let result = read_parts_inner(state, mode, body, headers, max_files, &mut written).await;
    if result.is_err() {
        cleanup_block_ids(state, &repo_id, &written, release_reservations).await;
    }
    result
}

async fn read_parts_inner(
    state: &AppState,
    mode: IntakeMode,
    body: axum::body::Body,
    headers: &HeaderMap,
    max_files: usize,
    written: &mut Vec<String>,
) -> Result<Parts, AppError> {
    let boundary = extract_multipart_boundary(headers)?;
    let mut multipart = multer::Multipart::new(body.into_data_stream(), boundary);
    let content_range = headers.get("content-range").and_then(|v| v.to_str().ok());

    let mut parts = Parts {
        fields: HashMap::new(),
        files: Vec::new(),
        chunked: None,
        blocks: Vec::new(),
        has_file: false,
    };
    while let Some(mut field) = multipart
        .next_field()
        .await
        .map_err(|e| AppError::Internal(format!("multipart error: {e}")))?
    {
        let name = field.name().unwrap_or("").to_string();
        if name != "file" {
            // The block endpoint used to cap field values at 1 MiB and answer 400
            // for non-UTF-8; keep that shape, which `field.text()` would not.
            let value = if matches!(&mode, IntakeMode::Blocks { .. }) {
                let raw = read_field_limited(&mut field, MAX_SMALL_BODY_BYTES).await?;
                String::from_utf8(raw)
                    .map_err(|_| AppError::BadRequest("invalid utf-8 field value".into()))?
            } else {
                field
                    .text()
                    .await
                    .map_err(|e| AppError::Internal(format!("multipart field error: {e}")))?
            };
            parts.fields.insert(name, value);
            continue;
        }
        let seen = parts.files.len() + usize::from(parts.chunked.is_some());
        if seen >= max_files {
            return Err(AppError::BadRequest("More files in one request".into()));
        }
        let default_name = match &mode {
            IntakeMode::Stream { default_name, .. } => *default_name,
            _ => "unknown",
        };
        let filename = field.file_name().unwrap_or(default_name).to_string();
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
            IntakeMode::Stream {
                repo_id, enc_key, ..
            } => {
                let key = enc_key.as_ref().map(|(k, i)| (k.as_slice(), i.as_slice()));
                let mut new_block_ids = Vec::new();
                let streamed = stream_file_into_blocks(
                    state.block_store.clone(),
                    repo_id,
                    field,
                    key,
                    &mut new_block_ids,
                )
                .await;
                // Whatever landed is cleanup material even if the field failed.
                written.extend(new_block_ids.iter().cloned());
                let (block_ids, total_size) = streamed?;
                parts.files.push(PartFile::Ingested(UploadedFile {
                    filename,
                    block_ids,
                    total_size,
                    new_block_ids,
                    mtime: None,
                }));
            }
            IntakeMode::Blocks { repo_id, owner } => {
                parts.has_file = true;
                let block_id = field.file_name().unwrap_or("").to_string();
                let data = read_field_limited(&mut field, MAX_BLOCK_UPLOAD_BYTES).await?;
                if !block_id.is_empty()
                    && !data.is_empty()
                    && crate::handler::block::write_client_block(
                        state, repo_id, *owner, &block_id, &data,
                    )
                    .await?
                {
                    written.push(block_id.clone());
                    parts.blocks.push(block_id);
                }
            }
        }
    }
    Ok(parts)
}

/// Buffer one multipart field under a byte cap, rejecting an oversized part from
/// its Content-Length or from the bytes actually read.
async fn read_field_limited(
    field: &mut multer::Field<'_>,
    limit: usize,
) -> Result<Vec<u8>, AppError> {
    if let Some(len) = field
        .headers()
        .get(axum::http::header::CONTENT_LENGTH)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<usize>().ok())
        && len > limit
    {
        return Err(AppError::ContentTooLarge);
    }
    let mut out = Vec::with_capacity(limit.min(1 << 20));
    while let Some(chunk) = field
        .chunk()
        .await
        .map_err(|e| AppError::Internal(format!("multipart field read error: {e}")))?
    {
        if out.len() + chunk.len() > limit {
            return Err(AppError::ContentTooLarge);
        }
        out.extend_from_slice(&chunk);
    }
    Ok(out)
}

/// A byte stream fed to the CDC chunker.
type ChunkStream<'a> = futures::stream::BoxStream<'a, Result<bytes::Bytes, AppError>>;

/// The block-streaming half for one multipart file part: the shared CDC ingest
/// (seafile's fixed 8 MiB avg / 6 MiB min / 10 MiB max sizes), with errors
/// already mapped to `AppError` so a cancelled field keeps its 4xx.
async fn stream_file_into_blocks(
    store: infra::storage::DynBlockStorage,
    repo_id: &str,
    field: multer::Field<'_>,
    enc_key: Option<(&[u8], &[u8])>,
    new_block_ids: &mut Vec<String>,
) -> Result<(Vec<String>, i64), AppError> {
    use futures::StreamExt;

    let stream: ChunkStream<'_> = Box::pin(
        field.map(|chunk| chunk.map_err(|e| AppError::Internal(format!("file read error: {e}")))),
    );
    crate::fs::core::FileOps::ingest_chunk_stream(&store, repo_id, stream, enc_key, new_block_ids)
        .await
}

/// CDC a staged file into the block store (called only after authorization).
async fn ingest_staged_file(
    store: infra::storage::DynBlockStorage,
    repo_id: &str,
    path: std::path::PathBuf,
    enc_key: Option<(&[u8], &[u8])>,
    new_block_ids: &mut Vec<String>,
) -> Result<(Vec<String>, i64), AppError> {
    use futures::StreamExt;

    let file = tokio::fs::File::open(&path)
        .await
        .map_err(|e| AppError::Internal(format!("open staged file: {e}")))?;
    let stream: ChunkStream<'_> = Box::pin(
        tokio_util::io::ReaderStream::new(file)
            .map(|chunk| chunk.map_err(|e| AppError::Internal(format!("read staged file: {e}")))),
    );
    crate::fs::core::FileOps::ingest_chunk_stream(&store, repo_id, stream, enc_key, new_block_ids)
        .await
}

/// A multipart file staged on disk before authorization.
///
/// The no-token web endpoints carry the repository id inside the multipart body,
/// so permission can only be checked after parsing; staging the bytes keeps them
/// out of the global block store until then, and the guard removes the file on
/// drop.
pub(crate) struct StagedUpload(Option<std::path::PathBuf>);

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
pub(crate) struct UploadedFile {
    pub(crate) filename: String,
    pub(crate) block_ids: Vec<String>,
    pub(crate) total_size: i64,
    pub(crate) new_block_ids: Vec<String>,
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

/// `POST /update-api/` 鈥?update an existing file from the web UI (no token),
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
    let range = headers.get("content-range").and_then(|v| v.to_str().ok());
    // An update carries exactly one file part.
    let parts = read_update_body(&state, None, body, &headers).await?;

    let repo_id = parts.fields.get("repo_id").cloned().unwrap_or_default();
    let file_path = parts
        .fields
        .get("p")
        .or_else(|| parts.fields.get("path"))
        .cloned()
        .unwrap_or_default();
    let csrf_token = parts.fields.get("csrf_token").cloned();

    if file_path.is_empty() {
        return Err(AppError::BadRequest("Path is missing.".into()));
    }
    if !file_path.starts_with('/') {
        return Err(AppError::BadRequest("Path is missing or invalid.".into()));
    }

    let auth = AuthContext::from_session(&state, &user, &repo_id, csrf_token.as_deref()).await?;

    let target = resolve_existing_target(None, &file_path, "")?;
    require_target_exists(&state, &auth.repo_id, &target.dir, &target.name).await?;

    let mut guard = UncommittedBlocks::new(
        state.clone(),
        auth.repo_id.clone(),
        parts.written_block_ids(),
        false,
    );
    let finished = finish_existing(
        &state,
        &auth,
        parts,
        &target,
        range,
        ReplyStyle::Array,
        &mut guard,
    )
    .await?;
    guard.disarm();
    Ok(finished.response)
}

/// `POST /update-aj/` 鈥?AJAX file update from the Seahub web UI (no token),
/// taking `file`, `repo_id` and `target_file` (the full target path).
pub async fn update_aj(
    user: WebUser,
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    body: axum::body::Body,
) -> Result<axum::response::Response, AppError> {
    // See `update_api`: an update replaces the file, so only the commit-time
    // size delta is a correct quota check.
    let range = headers.get("content-range").and_then(|v| v.to_str().ok());
    // An update carries exactly one file part.
    let parts = read_update_body(&state, None, body, &headers).await?;

    let repo_id = parts
        .fields
        .get("repo_id")
        .ok_or_else(|| AppError::BadRequest("repo_id required".into()))?;
    let target_file = parts
        .fields
        .get("target_file")
        .ok_or_else(|| AppError::BadRequest("target_file required".into()))?;

    let auth = AuthContext::from_session(
        &state,
        &user,
        repo_id,
        parts.fields.get("csrf_token").map(|s| s.as_str()),
    )
    .await?;

    let target = resolve_existing_target(None, target_file, "")?;
    require_target_exists(&state, &auth.repo_id, &target.dir, &target.name).await?;

    let mut guard = UncommittedBlocks::new(
        state.clone(),
        auth.repo_id.clone(),
        parts.written_block_ids(),
        false,
    );
    let finished = finish_existing(
        &state,
        &auth,
        parts,
        &target,
        range,
        ReplyStyle::Array,
        &mut guard,
    )
    .await?;
    guard.disarm();
    Ok(finished.response)
}

/// `POST /upload-aj/{token}` 鈥?upload with an upload-link token, as sent by the
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
    let range = headers.get("content-range").and_then(|v| v.to_str().ok());
    let parts = read_upload_body(&state, Some(&auth), body, &headers).await?;

    // The public upload page posts only the `file` part and relies on the minted
    // token for the directory, so a body without `parent_dir` keeps that default
    // instead of answering 400. The token's directory is still clamped, never
    // widened.
    let query = query_params(&uri);
    // Created before target resolution: a malformed parent_dir or name must not
    // leave the streamed blocks behind.
    let mut guard = UncommittedBlocks::new(
        state.clone(),
        auth.repo_id.clone(),
        parts.written_block_ids(),
        false,
    );
    let target = create_target(Some(auth.token()), &parts.fields, &query)?;

    let finished = finish_create(
        &state,
        &auth,
        parts,
        &target,
        range,
        ReplyStyle::Array,
        &mut guard,
    )
    .await?;
    guard.disarm();

    // The view counter only fires for a whole-file commit: the chunked branch
    // returned before it before the tail was shared.
    if finished.chunked {
        return Ok(finished.response);
    }
    if let Some(link_id) = auth.token().upload_link_id {
        upload_link_service::increment_upload_view_cnt(state.repos.clone(), link_id);
    }
    Ok(finished.response)
}

/// `POST /upload-api/{token}` 鈥?token-authenticated upload for the desktop
/// client, which sends a quoted multipart boundary that axum's extractor rejects.
pub async fn upload_api(
    State(state): State<Arc<AppState>>,
    Path(token): Path<String>,
    req: axum::http::Request<axum::body::Body>,
) -> Result<axum::response::Response, AppError> {
    let (request, body) = req.into_parts();
    let headers = request.headers;
    // The query has to be captured before the request body is consumed.
    let query = query_params(&request.uri);
    let range = headers.get("content-range").and_then(|v| v.to_str().ok());
    let auth = AuthContext::from_token(&state, &token, TokenKind::Upload).await?;
    // Resumable uploads reach this endpoint: the Qt desktop client fetches
    // `api2/repos/{id}/upload-link/` and chunks files over 100 MB
    // (`ReliablePostFileTask`). A chunk must go through the temp-file / assembly
    // path 鈥?streaming it straight into blocks would commit it as a whole file,
    // and that path is also where the declared size is reserved against quota.
    let parts = read_upload_body(&state, Some(&auth), body, &headers).await?;

    // Like the `-aj` token form, a missing `parent_dir` means the directory the
    // link was minted for; the desktop client sends it, older bodies may not.
    let mut guard = UncommittedBlocks::new(
        state.clone(),
        auth.repo_id.clone(),
        parts.written_block_ids(),
        false,
    );
    let target = create_target(Some(auth.token()), &parts.fields, &query)?;
    // Only `/upload-api` honours `ret-json`; official Android/desktop clients
    // read the plain-text body as the file id, so the default stays plain text.
    let ret_json = wants_ret_json(&query, &parts.fields);

    let finished = finish_create(
        &state,
        &auth,
        parts,
        &target,
        range,
        ReplyStyle::PlainOrJson { ret_json },
        &mut guard,
    )
    .await?;
    guard.disarm();
    Ok(finished.response)
}

/// `POST /update-api/{token}` 鈥?token-authenticated file overwrite for the
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
    let range = headers.get("content-range").and_then(|v| v.to_str().ok());
    // An update carries exactly one file part.
    let parts = read_update_body(&state, Some(&auth), body, &headers).await?;

    let target_file = parts
        .fields
        .get("target_file")
        .ok_or_else(|| AppError::BadRequest("No target_file given.".into()))?;
    let relative_path = parts
        .fields
        .get("relative_path")
        .cloned()
        .unwrap_or_default();
    // `/update-api` honours `ret-json`; without it the body is a plain id.
    let ret_json = wants_ret_json(&query, &parts.fields);

    // Scoped for consistency: an update token never carries `upload_link_id`
    // today, but this keeps the link-directory clamp if one ever does.
    let mut guard = UncommittedBlocks::new(
        state.clone(),
        auth.repo_id.clone(),
        parts.written_block_ids(),
        false,
    );
    let target = resolve_existing_target(Some(auth.token()), target_file, &relative_path)?;
    require_target_exists(&state, &auth.repo_id, &target.dir, &target.name).await?;

    let finished = finish_existing(
        &state,
        &auth,
        parts,
        &target,
        range,
        ReplyStyle::PlainOrJson { ret_json },
        &mut guard,
    )
    .await?;
    guard.disarm();
    Ok(finished.response)
}

/// `POST /update-aj/{token}` 鈥?token-based AJAX file update from the Seahub web
/// frontend, taking `file`, `target_file` and optional `relative_path`.
pub async fn update_aj_token(
    State(state): State<Arc<AppState>>,
    Path(token): Path<String>,
    headers: HeaderMap,
    body: axum::body::Body,
) -> Result<axum::response::Response, AppError> {
    let auth = AuthContext::from_token(&state, &token, TokenKind::Update).await?;

    let range = headers.get("content-range").and_then(|v| v.to_str().ok());
    // An update carries exactly one file part.
    let parts = read_update_body(&state, Some(&auth), body, &headers).await?;

    let target_file = parts
        .fields
        .get("target_file")
        .ok_or_else(|| AppError::BadRequest("No target_file given.".into()))?;
    let relative_path = parts
        .fields
        .get("relative_path")
        .cloned()
        .unwrap_or_default();
    // Scoped like every other token upload: an update token never carries
    // `upload_link_id` today, but this keeps the link-directory clamp if one
    // ever does (and matches `update_api_handler`).
    let mut guard = UncommittedBlocks::new(
        state.clone(),
        auth.repo_id.clone(),
        parts.written_block_ids(),
        false,
    );
    let target = resolve_existing_target(Some(auth.token()), target_file, &relative_path)?;
    require_target_exists(&state, &auth.repo_id, &target.dir, &target.name).await?;

    let finished = finish_existing(
        &state,
        &auth,
        parts,
        &target,
        range,
        ReplyStyle::Array,
        &mut guard,
    )
    .await?;
    guard.disarm();
    Ok(finished.response)
}

/// The commit-phase fields of a block upload.
struct BlockCommitForm {
    block_ids: Vec<String>,
    file_size: i64,
}

impl BlockCommitForm {
    /// Parse and sanity-check `blockids` / `file_size`: both are required, the
    /// list must be a JSON array, and only a zero-byte file may have no blocks.
    fn parse(fields: &HashMap<String, String>) -> Result<Self, AppError> {
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
        Ok(Self {
            block_ids,
            file_size,
        })
    }
}

/// Commit a block upload: validate the ids, verify the declared size, resolve
/// the target, write the file object and release the block reservations.
async fn commit_blocks(
    state: &AppState,
    info: &crate::service::auth::access_token::AccessToken,
    fields: &HashMap<String, String>,
    query: &HashMap<String, String>,
    block_ids: Vec<String>,
    file_size: i64,
    new_block_ids: Vec<String>,
) -> Result<axum::response::Response, AppError> {
    let uid = Some(info.user_id);
    // Reject malformed / path-traversal ids outright 鈥?a valid block id is
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
        let target = resolve_existing_target(Some(info), target_file, "")?;
        require_target_exists(state, &info.repo_id, &target.dir, &target.name).await?;
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
            compute_scoped_target_dir(info, parent_dir, relative_path)?,
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
        state,
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
                    crate::service::fs::quota::release_block_bytes(&state.repos, &info.repo_id, id)
                        .await;
                }
            }
            return Err(e);
        }
    };

    // The blocks are referenced by HEAD now; their uncommitted reservations end.
    crate::service::fs::quota::release_blocks(&state.repos, &info.repo_id, &block_ids).await;

    Ok(reply_committed(
        ReplyStyle::PlainOrJson {
            ret_json: wants_ret_json(query, fields),
        },
        &[committed],
    ))
}

/// Validate an `upload-blks` / `update-blks` token: op, active account, write
/// permission (which also names the quota owner) and the encryption refusal.
///
/// Block-level uploads name their own blocks (`filename` = block id, which must
/// equal `sha1(plaintext)`) and are committed by replaying that same
/// client-supplied id list. Server-side encryption would have to store the block
/// under `sha1(ciphertext)` instead, so the ids the client later commits would
/// not resolve. Upstream only ever used this API for encrypted libraries with
/// *client-side* encryption (a path the current iOS client no longer calls), so
/// encrypted libraries are refused here rather than silently storing plaintext
/// under a mismatched id.
async fn block_auth(
    state: &AppState,
    token: &str,
) -> Result<(crate::service::auth::access_token::AccessToken, i32), AppError> {
    let info = crate::service::auth::capability::validate_capability(
        state,
        token,
        crate::service::auth::capability::TokenPolicy {
            ops: &["upload-blks", "update-blks"],
            invalid_message: "invalid or expired upload token",
            wrong_op_message: "token not valid for block upload",
            // A block-upload token can also come from an upload link; the link's
            // own expiry is part of the capability.
            check_upload_link_expiry: true,
        },
    )
    .await?;

    // Re-check write permission: the token was issued against the caller's
    // membership, but that may have been revoked since (matches download). The
    // owner comes back from the same row so the per-block quota reservation does
    // not look it up again.
    let owner = crate::domain::permission::repo_write_owner(
        state.repos.member.as_ref(),
        &info.repo_id,
        info.user_id,
    )
    .await?;

    if let Some(repo_model) = state.repos.repo.find_by_id(&info.repo_id).await?
        && repo_model.encrypted != 0
    {
        return Err(AppError::BadRequest(
            "block upload is not supported for encrypted libraries".into(),
        ));
    }

    Ok((info, owner))
}

/// `POST /upload-blks-api/{token}` 鈥?block upload and commit.
///
/// Without `commitonly`, each `file` part is one block whose filename is its
/// SHA-1, verified before storing. With `commitonly`, the `parent_dir`,
/// `file_name`, `blockids` (JSON array), `file_size` and optional `replace`
/// fields assemble the file; `update-blks` only accepts the commit phase,
/// because upstream's update handler requires it and a block-only request would
/// otherwise look like an upload and answer success without touching the target.
pub async fn upload_blks_api(
    State(state): State<Arc<AppState>>,
    Path(token): Path<String>,
    req: axum::http::Request<axum::body::Body>,
) -> Result<axum::response::Response, AppError> {
    let (request, body) = req.into_parts();
    let headers = request.headers;
    let query = query_params(&request.uri);
    let (info, owner) = block_auth(&state, &token).await?;

    let parts = read_parts(
        &state,
        IntakeMode::Blocks {
            repo_id: info.repo_id.clone(),
            owner,
        },
        body,
        &headers,
        usize::MAX,
    )
    .await?;
    // Block writes reserve quota per block, so the guard releases reservations as
    // well as deleting blocks when the request fails before a commit references
    // them.
    let written = parts.written_block_ids();
    let mut guard = UncommittedBlocks::new(state.clone(), info.repo_id.clone(), written, true);
    let Parts {
        fields,
        blocks: new_block_ids,
        has_file,
        ..
    } = parts;

    let commit_only = fields.contains_key("commitonly") || query.contains_key("commitonly");
    if !commit_only && !has_file {
        return Err(AppError::BadRequest("No file in multipart form".into()));
    }
    // Upstream's update handler requires the commit phase.
    if info.op == "update-blks" && !commit_only {
        return Err(AppError::BadRequest(
            "Only commit supported for update-blks-api".into(),
        ));
    }

    if commit_only {
        let BlockCommitForm {
            block_ids,
            file_size,
        } = BlockCommitForm::parse(&fields)?;

        let response = commit_blocks(
            &state,
            &info,
            &fields,
            &query,
            block_ids,
            file_size,
            new_block_ids,
        )
        .await;
        if response.is_ok() {
            guard.disarm();
        }
        return response;
    }

    // A block-only upload is the first half of a two-request protocol: those
    // blocks are meant to stay pending for the later `commitonly` request, so the
    // cleanup guard must not delete them.
    guard.disarm();
    Ok(ok_json().into_response())
}

#[cfg(test)]
mod tests {
    use super::*;

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

        // Declared chunk is 4096 bytes, cap is 1024 鈥?rejected from the header.
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
