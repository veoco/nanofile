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
    Json,
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
use crate::fs::zip::{ZipFileEntry, collect_selected_entries, stream_zip};
use crate::middleware::auth::AuthUser;
use base::error::AppError;

// ── Data types ─────────────────────────────────────────────────────────

/// Task info stored per zip-token.
#[allow(dead_code)]
struct ZipTaskInfo {
    repo_id: String,
    files: Vec<ZipFileEntry>,
    // zip display name (without .zip extension)
    zip_name: String,
    created_at: i64,
}

// ── In-memory token store ──────────────────────────────────────────────

static ZIP_TASKS: OnceLock<Mutex<HashMap<String, ZipTaskInfo>>> = OnceLock::new();

/// TTL for an unconsumed zip task, and a hard cap on the in-memory map so a
/// flood of zip-task requests can't grow it without bound.
const ZIP_TASK_TTL_SECS: i64 = 3600;
const MAX_ZIP_TASKS: usize = 1000;

fn zip_tasks() -> &'static Mutex<HashMap<String, ZipTaskInfo>> {
    ZIP_TASKS.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Remove zip tasks older than `ZIP_TASK_TTL_SECS`. Called on new task
/// creation and periodically by the scheduler so abandoned tasks don't
/// accumulate.
pub fn cleanup_expired(now: i64) {
    if let Ok(mut tasks) = zip_tasks().lock() {
        tasks.retain(|_, t| now - t.created_at < ZIP_TASK_TTL_SECS);
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
    Json(payload): Json<ZipTaskRequest>,
) -> Result<JsonResponse<ZipTaskResponse>, AppError> {
    // Verify read permission
    crate::domain::permission::check_repo_read_permission(
        state.repos.member.as_ref(),
        &repo_id,
        auth.user_id,
    )
    .await?;

    if payload.dirents.is_empty() {
        return Err(AppError::BadRequest(
            "No entries specified for download".into(),
        ));
    }

    // Resolve head commit root
    let root_fs_id = infra::common::util::get_head_root_id(&state.db, &repo_id).await?;

    // Collect files (recursively for directories)
    let files = collect_selected_entries(
        &state.repos,
        &repo_id,
        &root_fs_id,
        &payload.parent_dir,
        &payload.dirents,
    )
    .await?;

    if files.is_empty() {
        return Err(AppError::NotFound("No files to download".into()));
    }

    let zip_name = determine_zip_name(&payload.parent_dir, &payload.dirents);
    let token = generate_token();
    let now = now_secs();

    // Purge abandoned tasks and enforce the in-memory cap.
    cleanup_expired(now);
    {
        let mut tasks = zip_tasks()
            .lock()
            .map_err(|_| AppError::Internal("zip task registry poisoned".into()))?;
        if tasks.len() >= MAX_ZIP_TASKS {
            return Err(AppError::TooManyRequests);
        }
        tasks.insert(
            token.clone(),
            ZipTaskInfo {
                repo_id: repo_id.clone(),
                files,
                zip_name,
                created_at: now,
            },
        );
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
        let mut tasks = zip_tasks()
            .lock()
            .map_err(|_| AppError::Internal("zip task registry poisoned".into()))?;
        tasks
            .remove(&token)
            .ok_or_else(|| AppError::NotFound("Zip task not found or expired".into()))?
    };

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

    let stream = stream_zip(state.block_store.clone(), task.files, dec_key);

    let mut headers = axum::http::HeaderMap::new();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/zip"),
    );
    headers.insert(
        header::CONTENT_DISPOSITION,
        HeaderValue::from_str(&format!("attachment; filename=\"{}\"", zip_filename))
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
