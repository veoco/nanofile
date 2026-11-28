//! ZIP download module — Seafile-compatible batch download as a streaming zip.
//!
//! `POST /api/v2.1/repos/{repo_id}/zip-task/` mints a token, and
//! `GET /zip/{token}` streams the archive.
//!
//! The stream uses **data descriptors** (CRC-32 and sizes written *after* the
//! compressed data) so entries stream without seeking back to patch local file
//! headers; see `async_zip::base::write::entry_stream::EntryStreamWriter::close()`.

use axum::{
    body::Body,
    extract::{Path, State},
    http::{HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
};
use serde::Deserialize;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::AppState;
use crate::fs::zip::{ZipLimits, acquire_zip_permit, collect_selected_entries, stream_zip};
use crate::middleware::auth::AuthUser;
use crate::service::upload::zip_task::{MAX_ZIP_TASKS, ZipTaskInfo, cleanup_expired, mint, take};
use base::error::AppError;

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

/// Determine the zip filename (without extension) based on the request.
fn determine_zip_name(dirents: &[String]) -> String {
    if dirents.len() == 1 {
        dirents[0].trim_end_matches('/').to_string()
    } else {
        // Multi-file download keeps seahub's date-based convention.
        let now = chrono::Local::now();
        format!("documents-export-{}", now.format("%Y-%m-%d"))
    }
}

/// Parse the `zip-task` payload from either a JSON body or `multipart/form-data`.
///
/// The official frontend posts `FormData` with a repeated `dirents` field
/// (`seahub/api2/endpoints/zip_task.py`), so a JSON-only extractor made the
/// browser's multi-select download fail with 415.
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

/// `POST /api/v2.1/repos/{repo_id}/zip-task/` — mint a `zip_token` for
/// `GET /zip/{token}`, taking `parent_dir` and one or more `dirents`.
pub async fn zip_task_handler(
    auth: AuthUser,
    State(state): State<Arc<AppState>>,
    Path(repo_id): Path<String>,
    headers: axum::http::HeaderMap,
    body: axum::body::Body,
) -> Result<JsonResponse<ZipTaskResponse>, AppError> {
    let mut payload = parse_zip_task_request(&headers, body).await?;
    crate::service::permission::check_repo_read_permission(
        state.repos.member.as_ref(),
        &repo_id,
        auth.user_id,
    )
    .await?;

    // Bound and de-duplicate the requested names before any tree walk: each
    // directory triggers a recursive traversal while holding a global ZIP permit.
    crate::handler::sanitize_dirent_list(&mut payload.dirents)?;

    if payload.dirents.is_empty() {
        return Err(AppError::BadRequest(
            "No entries specified for download".into(),
        ));
    }

    let root_fs_id = crate::fs::core::repo_head(&state.repos, &repo_id)
        .await?
        .require_root()?;

    // Gate the expensive recursive collection phase so a flood of zip tokens
    // cannot bypass the global concurrency cap; the guard holds for the walk.
    let _permit = acquire_zip_permit().await?;

    // Collect files recursively, bounded by the per-archive entry/byte caps (429).
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

    let zip_name = determine_zip_name(&payload.dirents);
    let token = generate_token();
    let now = now_secs();

    // Purge abandoned tasks, then enforce the registry's count/byte budgets.
    let _ = cleanup_expired(now);
    let max_bytes = state.config().storage.max_zip_task_bytes as usize;
    let stored = mint(
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
    )
    .map_err(AppError::Internal)?;
    if !stored {
        return Err(AppError::TooManyRequests);
    }

    Ok(JsonResponse(ZipTaskResponse { zip_token: token }))
}

/// `GET /zip/{token}` — stream the archive for a previously minted token as
/// `application/zip` with a `Content-Disposition: attachment` filename.
pub async fn zip_download_handler(
    State(state): State<Arc<AppState>>,
    Path(token): Path<String>,
) -> Result<Response, AppError> {
    let task = take(&token)
        .map_err(AppError::Internal)?
        .ok_or_else(|| AppError::NotFound("Zip task not found or expired".into()))?;

    // The token outlives membership, so re-check the requester the same way
    // `/api/transfers/download/{token}` does: a user removed from the library
    // (or deactivated) between `POST /zip-task/` and this GET must not receive
    // the archive.
    let permitted = crate::service::permission::check_repo_read_permission(
        state.repos.member.as_ref(),
        &task.repo_id,
        task.user_id,
    )
    .await
    .is_ok();
    let active = super::download::token_user_active(&state, task.user_id).await?;
    if !permitted || !active {
        return Err(AppError::NotFound("Zip task not found or expired".into()));
    }

    // The token carries no user identity, so an encrypted repo cannot be served here.
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
            return Err(AppError::BadRequest(
                "Zip download for encrypted repos is not supported via token. \
                 Use the direct download API instead."
                    .into(),
            ));
        }
    };

    let zip_filename = format!("{}.zip", task.zip_name);

    // Held for the writer task's full lifecycle once moved into `stream_zip`.
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
        HeaderValue::from_str(&crate::http::download::content_disposition(
            &zip_filename,
            true,
        ))
        .unwrap_or_else(|_| HeaderValue::from_static("attachment")),
    );

    Ok((StatusCode::OK, headers, Body::from_stream(stream)).into_response())
}

/// Wraps a serializable value into an `axum::Json` response.
pub struct JsonResponse<T: serde::Serialize>(pub T);

impl<T: serde::Serialize> IntoResponse for JsonResponse<T> {
    fn into_response(self) -> Response {
        axum::Json(self.0).into_response()
    }
}
