//! ZIP download module — Seafile-compatible batch download as streaming zip.
//!
//! Implements:
//! - `POST /api/v2.1/repos/{repo_id}/zip-task/` — request a zip download token
//! - `GET /zip/{token}` — download the zip (streamed via `async_zip` + data descriptors)
//!
//! The ZIP stream uses **data descriptors** (`GeneralPurposeFlag.data_descriptor = true`)
//! so that each file entry's CRC-32 and sizes are written *after* the compressed data,
//! allowing true streaming without seeking back to patch the local file header.
//! See `async_zip::base::write::entry_stream::EntryStreamWriter::close()` for the
//! data-descriptor write (CRC-32 → compressed size → uncompressed size).

use axum::{
    body::Body,
    extract::{Path, State},
    http::{HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
};
use serde::Deserialize;
use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::AppState;
use crate::fs::zip::{
    ZipFileEntry, ZipLimits, acquire_zip_permit, collect_selected_entries, stream_zip,
};
use crate::middleware::auth::AuthUser;
use base::error::AppError;

// ── Data types ─────────────────────────────────────────────────────────

/// Task info stored per zip-token.
#[allow(dead_code)]
struct ZipTaskInfo {
    repo_id: String,
    /// Who requested the archive. Re-checked when the token is consumed, so a
    /// user whose access was revoked (or whose account was deactivated) during
    /// the token's one-hour TTL cannot still download the archive.
    user_id: i32,
    files: Vec<ZipFileEntry>,
    // zip display name (without .zip extension)
    zip_name: String,
    created_at: i64,
}

// ── In-memory token store ──────────────────────────────────────────────

/// Unconsumed zip-download tokens, bounded by **both** a count and a byte
/// budget.
///
/// The byte budget is the one that decides memory use: a token carries the full
/// entry list, and `ZipFileEntry::block_ids` is a per-file vector of 40-char
/// ids, so `max_zip_entries` entries times concurrently-held tokens reaches
/// gigabytes on a directory of large files. A count cap alone would not stop
/// that.
#[derive(Default)]
struct ZipTaskRegistry {
    tasks: HashMap<String, ZipTaskInfo>,
    /// Sum of [`ZipTaskInfo::entry_bytes`] over `tasks`, kept in step with
    /// every insert and removal so the budget check is O(1).
    bytes: usize,
}

impl ZipTaskInfo {
    /// Rough heap footprint of one registry entry, used only for the byte
    /// budget. Deliberately an over-estimate: it counts the struct, the map key
    /// and every owned string (including each block id).
    fn entry_bytes(token: &str, info: &Self) -> usize {
        let files: usize = info
            .files
            .iter()
            .map(|f| {
                std::mem::size_of::<ZipFileEntry>()
                    + f.path_in_zip.len()
                    + f.block_ids
                        .iter()
                        .map(|b| b.len() + std::mem::size_of::<String>())
                        .sum::<usize>()
            })
            .sum();
        std::mem::size_of::<Self>()
            + token.len()
            + std::mem::size_of::<String>()
            + info.repo_id.len()
            + info.zip_name.len()
            + files
    }
}

impl ZipTaskRegistry {
    /// Drop tokens older than [`ZIP_TASK_TTL_SECS`], returning their bytes to
    /// the budget.
    fn sweep(&mut self, now: i64) {
        let mut freed = 0usize;
        self.tasks.retain(|token, task| {
            let keep = now - task.created_at < ZIP_TASK_TTL_SECS;
            if !keep {
                freed += ZipTaskInfo::entry_bytes(token, task);
            }
            keep
        });
        self.bytes = self.bytes.saturating_sub(freed);
    }

    /// Insert a token, evicting oldest-first until both budgets hold.
    ///
    /// Returns `false` when the entry alone exceeds `max_bytes`, which is a
    /// request that can never be satisfied rather than one to wait for.
    fn insert(
        &mut self,
        token: String,
        info: ZipTaskInfo,
        max_count: usize,
        max_bytes: usize,
    ) -> bool {
        let bytes = ZipTaskInfo::entry_bytes(&token, &info);
        if max_bytes > 0 && bytes > max_bytes {
            return false;
        }
        loop {
            let over_count = max_count > 0 && self.tasks.len() >= max_count;
            let over_bytes = max_bytes > 0 && self.bytes + bytes > max_bytes;
            if !over_count && !over_bytes {
                break;
            }
            let Some(oldest) = self
                .tasks
                .iter()
                .min_by_key(|(_, task)| task.created_at)
                .map(|(token, _)| token.clone())
            else {
                break;
            };
            if let Some(removed) = self.tasks.remove(&oldest) {
                self.bytes = self
                    .bytes
                    .saturating_sub(ZipTaskInfo::entry_bytes(&oldest, &removed));
            }
        }
        self.bytes += bytes;
        self.tasks.insert(token, info);
        true
    }

    /// Consume a token, returning its bytes to the budget.
    fn take(&mut self, token: &str) -> Option<ZipTaskInfo> {
        let removed = self.tasks.remove(token)?;
        self.bytes = self
            .bytes
            .saturating_sub(ZipTaskInfo::entry_bytes(token, &removed));
        Some(removed)
    }
}

static ZIP_TASKS: OnceLock<Mutex<ZipTaskRegistry>> = OnceLock::new();

/// TTL for an unconsumed zip task, and a hard cap on how many may be held at
/// once. The byte budget in [`ZipTaskRegistry`] is the binding limit in
/// practice.
const ZIP_TASK_TTL_SECS: i64 = 3600;
const MAX_ZIP_TASKS: usize = 1000;

fn zip_tasks() -> &'static Mutex<ZipTaskRegistry> {
    ZIP_TASKS.get_or_init(|| Mutex::new(ZipTaskRegistry::default()))
}

/// Remove zip tasks older than `ZIP_TASK_TTL_SECS`. Called on new task
/// creation and periodically by the scheduler so abandoned tasks don't
/// accumulate.
pub fn cleanup_expired(now: i64) {
    if let Ok(mut registry) = zip_tasks().lock() {
        registry.sweep(now);
    }
}

fn generate_token() -> String {
    uuid::Uuid::new_v4().to_string()
}

fn now_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}

/// JSON payload for `POST zip-task/`.
#[derive(Deserialize)]
pub struct ZipTaskRequest {
    pub parent_dir: String,
    /// File/folder names within `parent_dir`.
    pub dirents: Vec<String>,
}

/// JSON response for `POST zip-task/`.
#[derive(serde::Serialize)]
pub struct ZipTaskResponse {
    pub zip_token: String,
}

// ── Helpers ────────────────────────────────────────────────────────────

/// Determine the zip filename (without extension) based on the request.
#[allow(unused_variables)]
fn determine_zip_name(parent_dir: &str, dirents: &[String]) -> String {
    if dirents.len() == 1 {
        // Single directory download → use directory name
        dirents[0].trim_end_matches('/').to_string()
    } else {
        // Multi-file download → use date-based name (matching seahub convention)
        let now = chrono::Local::now();
        format!("documents-export-{}", now.format("%Y-%m-%d"))
    }
}

// ── Handlers ───────────────────────────────────────────────────────────

/// Parse the `zip-task` payload from either a JSON body or `multipart/form-data`.
///
/// The official web frontend posts `FormData` with `parent_dir` and a repeated
/// `dirents` field (`seafile-api.js`, `seahub/api2/endpoints/zip_task.py`), so a
/// JSON-only extractor made the browser's multi-select download fail with 415.
async fn parse_zip_task_request(
    headers: &axum::http::HeaderMap,
    body: axum::body::Body,
) -> Result<ZipTaskRequest, AppError> {
    let content_type = headers
        .get(axum::http::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    if content_type.starts_with("multipart/form-data") {
        let boundary = multer::parse_boundary(content_type)
            .map_err(|e| AppError::BadRequest(format!("invalid multipart boundary: {e}")))?;
        let mut mp = multer::Multipart::new(body.into_data_stream(), boundary);
        let mut parent_dir = "/".to_string();
        let mut dirents: Vec<String> = Vec::new();
        while let Some(field) = mp
            .next_field()
            .await
            .map_err(|e| AppError::BadRequest(format!("multipart error: {e}")))?
        {
            let name = field.name().unwrap_or("").to_string();
            let text = field
                .text()
                .await
                .map_err(|e| AppError::BadRequest(format!("multipart field error: {e}")))?;
            match name.as_str() {
                "parent_dir" => parent_dir = text,
                "dirents" => dirents.push(text),
                _ => {}
            }
        }
        return Ok(ZipTaskRequest {
            parent_dir,
            dirents,
        });
    }

    let bytes = axum::body::to_bytes(body, crate::handler::MAX_SMALL_BODY_BYTES)
        .await
        .map_err(|e| AppError::BadRequest(e.to_string()))?;
    serde_json::from_slice(&bytes)
        .map_err(|e| AppError::BadRequest(format!("invalid JSON body: {e}")))
}

/// `POST /api/v2.1/repos/{repo_id}/zip-task/`
///
/// Accepts form data:
/// - `parent_dir` — the directory containing the items to download
/// - `dirents` — one or more file/folder names within `parent_dir`
///
/// Returns `{ "zip_token": "<uuid>" }` which the client can then pass to
/// `GET /zip/{token}` to receive the actual zip stream.
pub async fn zip_task_handler(
    auth: AuthUser,
    State(state): State<Arc<AppState>>,
    Path(repo_id): Path<String>,
    headers: axum::http::HeaderMap,
    body: axum::body::Body,
) -> Result<JsonResponse<ZipTaskResponse>, AppError> {
    let mut payload = parse_zip_task_request(&headers, body).await?;
    // Verify read permission
    crate::domain::permission::check_repo_read_permission(
        state.repos.member.as_ref(),
        &repo_id,
        auth.user_id,
    )
    .await?;

    // Bound and de-duplicate the requested names before any tree walk: each
    // directory name triggers a full recursive traversal while holding one of
    // the (few) global ZIP permits, so an unbounded list is a memory and
    // availability hazard, not just wasted work.
    crate::handler::sanitize_dirent_list(&mut payload.dirents)?;

    if payload.dirents.is_empty() {
        return Err(AppError::BadRequest(
            "No entries specified for download".into(),
        ));
    }

    // Resolve head commit root
    let root_fs_id = infra::common::util::get_head_root_id(&state.db, &repo_id).await?;

    // Gate the expensive collection phase (recursive DB traversal) so a flood
    // of zip tokens can't bypass the global concurrency cap. The permit is held
    // as a scoped guard for the duration of the walk and dropped after.
    let _permit = acquire_zip_permit().await?;

    // Collect files (recursively for directories), bounded by the configured
    // per-archive entry/byte caps (429 when exceeded).
    let files = collect_selected_entries(
        &state.repos,
        &repo_id,
        &root_fs_id,
        &payload.parent_dir,
        &payload.dirents,
        ZipLimits {
            max_entries: state.config().storage.max_zip_entries,
            max_bytes: state.config().storage.max_zip_bytes,
        },
    )
    .await?;

    if files.is_empty() {
        return Err(AppError::NotFound("No files to download".into()));
    }

    let zip_name = determine_zip_name(&payload.parent_dir, &payload.dirents);
    let token = generate_token();
    let now = now_secs();

    // Purge abandoned tasks and enforce the count/byte budgets.
    cleanup_expired(now);
    {
        let max_bytes = state.config().storage.max_zip_task_bytes as usize;
        let mut registry = zip_tasks()
            .lock()
            .map_err(|_| AppError::Internal("zip task registry poisoned".into()))?;
        let stored = registry.insert(
            token.clone(),
            ZipTaskInfo {
                repo_id: repo_id.clone(),
                user_id: auth.user_id,
                files,
                zip_name,
                created_at: now,
            },
            MAX_ZIP_TASKS,
            max_bytes,
        );
        if !stored {
            return Err(AppError::TooManyRequests);
        }
    }

    Ok(JsonResponse(ZipTaskResponse { zip_token: token }))
}

/// `GET /zip/{token}`
///
/// Streams the zip archive for a previously requested zip-task token.
/// The response has:
/// - `Content-Type: application/zip`
/// - `Content-Disposition: attachment; filename="<name>.zip"`
pub async fn zip_download_handler(
    State(state): State<Arc<AppState>>,
    Path(token): Path<String>,
) -> Result<Response, AppError> {
    // Look up the task
    let task = {
        let mut registry = zip_tasks()
            .lock()
            .map_err(|_| AppError::Internal("zip task registry poisoned".into()))?;
        registry
            .take(&token)
            .ok_or_else(|| AppError::NotFound("Zip task not found or expired".into()))?
    };

    // The token outlives membership, so re-check the requester the same way
    // `/download-api` does: a user removed from the library (or deactivated)
    // between `POST /zip-task/` and this GET must not receive the archive.
    if !crate::domain::permission::check_repo_read_permission(
        state.repos.member.as_ref(),
        &task.repo_id,
        task.user_id,
    )
    .await
    .is_ok()
        || !state
            .repos
            .user
            .find_by_id(task.user_id)
            .await?
            .is_some_and(|u| u.is_active)
    {
        return Err(AppError::NotFound("Zip task not found or expired".into()));
    }

    // Check if repo is encrypted and if password is set (for the user who created the task)
    // For simplicity with token-based access, we handle this case separately.
    // The token-based download doesn't carry user identity, so encrypted repos
    // without cached password will fail here.
    let dec_key: Option<(Vec<u8>, Vec<u8>)> = {
        let repo_model = state
            .repos
            .repo
            .find_by_id(&task.repo_id)
            .await?
            .ok_or_else(|| AppError::NotFound("Repository not found".into()))?;

        if repo_model.encrypted == 0 {
            None
        } else {
            // Encrypted repo: try to get from password manager
            // We don't have a user_id here, so we check all cached passwords.
            // This is a limitation — for encrypted repos, the two-step token flow
            // won't work. Users should use the direct download API instead.
            // For now, return an error for encrypted repos.
            return Err(AppError::BadRequest(
                "Zip download for encrypted repos is not supported via token. \
                 Use the direct download API instead."
                    .into(),
            ));
        }
    };

    let zip_filename = format!("{}.zip", task.zip_name);

    // Acquire a permit for the streaming phase; moved into stream_zip and held
    // for the writer task's full lifecycle.
    let permit = acquire_zip_permit().await?;
    let stream = stream_zip(
        task.repo_id.clone(),
        state.block_store.clone(),
        task.files,
        dec_key,
        permit,
    );

    let mut headers = axum::http::HeaderMap::new();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/zip"),
    );
    headers.insert(
        header::CONTENT_DISPOSITION,
        HeaderValue::from_str(&crate::fs::core::download::content_disposition(
            &zip_filename,
            true,
        ))
        .unwrap_or_else(|_| HeaderValue::from_static("attachment")),
    );

    Ok((StatusCode::OK, headers, Body::from_stream(stream)).into_response())
}

// ── JsonResponse wrapper ───────────────────────────────────────────────

/// Wraps a serializable value into an `axum::Json` response.
pub struct JsonResponse<T: serde::Serialize>(pub T);

impl<T: serde::Serialize> IntoResponse for JsonResponse<T> {
    fn into_response(self) -> Response {
        axum::Json(self.0).into_response()
    }
}

#[cfg(test)]
mod registry_tests {
    use super::*;

    fn entry(path: &str, blocks: usize) -> ZipFileEntry {
        ZipFileEntry {
            path_in_zip: path.to_string(),
            block_ids: (0..blocks).map(|i| format!("{i:040x}")).collect(),
            size: 1,
        }
    }

    fn info(files: Vec<ZipFileEntry>, created_at: i64) -> ZipTaskInfo {
        ZipTaskInfo {
            repo_id: "repo".to_string(),
            user_id: 1,
            files,
            zip_name: "z".to_string(),
            created_at,
        }
    }

    fn bytes_of(token: &str, files: Vec<ZipFileEntry>) -> usize {
        ZipTaskInfo::entry_bytes(token, &info(files, 0))
    }

    /// The byte budget — not the token count — is what bounds memory: once it
    /// is reached, the oldest tokens are evicted to make room.
    #[test]
    fn byte_budget_evicts_the_oldest_tokens() {
        let mut registry = ZipTaskRegistry::default();
        let files = || vec![entry("a.txt", 40)];
        let one = bytes_of("t0", files());

        assert!(registry.insert("t0".into(), info(files(), 100), 0, one * 2));
        assert!(registry.insert("t1".into(), info(files(), 200), 0, one * 2));
        assert!(registry.insert("t2".into(), info(files(), 300), 0, one * 2));

        assert_eq!(registry.tasks.len(), 2, "the budget holds two tokens");
        assert!(!registry.tasks.contains_key("t0"), "the oldest is evicted");
        assert!(registry.tasks.contains_key("t1"));
        assert!(registry.tasks.contains_key("t2"));
        assert!(registry.bytes <= one * 2, "the tally stays within budget");
    }

    /// An archive whose listing alone exceeds the whole budget can never be
    /// served from memory, so it is refused rather than evicting everything
    /// else to make room for something that still would not fit.
    #[test]
    fn an_oversized_token_is_refused_without_evicting() {
        let mut registry = ZipTaskRegistry::default();
        let one = bytes_of("small", vec![entry("a.txt", 4)]);
        assert!(registry.insert("small".into(), info(vec![entry("a.txt", 4)], 1), 0, one * 2));

        let big = vec![entry("big.bin", 4000)];
        assert!(
            !registry.insert("big".into(), info(big, 2), 0, one * 2),
            "an entry larger than the whole budget must be refused"
        );
        assert!(
            registry.tasks.contains_key("small"),
            "refusing must not evict the tokens already stored"
        );
    }

    /// The count cap evicts rather than failing, and consuming a token returns
    /// its bytes to the budget.
    #[test]
    fn count_cap_evicts_and_take_frees_bytes() {
        let mut registry = ZipTaskRegistry::default();
        assert!(registry.insert("t0".into(), info(vec![entry("a.txt", 4)], 1), 1, 0));
        assert_eq!(registry.tasks.len(), 1);
        let with_one = registry.bytes;

        assert!(registry.insert("t1".into(), info(vec![entry("a.txt", 4)], 2), 1, 0));
        assert_eq!(registry.tasks.len(), 1);
        assert!(
            !registry.tasks.contains_key("t0"),
            "the count cap evicts the oldest"
        );
        assert_eq!(registry.bytes, with_one, "the tally follows the eviction");

        assert!(registry.take("t1").is_some());
        assert_eq!(registry.bytes, 0, "consuming returns the bytes");
        assert!(registry.take("t1").is_none());
    }

    /// Expiry returns the bytes too, so the budget cannot be pinned by
    /// abandoned tokens.
    #[test]
    fn sweep_drops_expired_tokens_and_their_bytes() {
        let mut registry = ZipTaskRegistry::default();
        let base = 1_000;
        let fresh_at = base + ZIP_TASK_TTL_SECS;
        // The token itself is part of an entry's footprint, so the two sizes
        // differ; compute each from the token it is actually stored under.
        let old_bytes = ZipTaskInfo::entry_bytes("old", &info(vec![entry("a.txt", 4)], base));
        let fresh_bytes =
            ZipTaskInfo::entry_bytes("fresh", &info(vec![entry("a.txt", 4)], fresh_at));

        registry.insert("old".into(), info(vec![entry("a.txt", 4)], base), 0, 0);
        registry.insert(
            "fresh".into(),
            info(vec![entry("a.txt", 4)], fresh_at),
            0,
            0,
        );
        assert_eq!(registry.bytes, old_bytes + fresh_bytes);

        registry.sweep(fresh_at + 1);

        assert_eq!(registry.tasks.len(), 1);
        assert!(
            !registry.tasks.contains_key("old"),
            "expired token is dropped"
        );
        assert!(registry.tasks.contains_key("fresh"));
        assert_eq!(
            registry.bytes, fresh_bytes,
            "the expired token's bytes are freed"
        );
    }
}
