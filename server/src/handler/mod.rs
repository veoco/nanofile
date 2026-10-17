//! HTTP handler layer — thin adapters between axum and the service layer.
//!
//! REST handlers are flat files grouped by domain; the sync protocol (`/seafhttp/`)
//! and file serving have subdirectories because of their distinct auth patterns.

use axum::Json;
use base::error::AppError;

pub fn ok_json() -> Json<serde_json::Value> {
    Json(serde_json::json!({"success": true}))
}

/// Upper bound for pure-metadata bodies (login, file/dir/repo/trash ops).
pub const MAX_SMALL_BODY_BYTES: usize = 1024 * 1024;

/// Upper bound for a single content-addressed block: Seafile CDC produces blocks up
/// to 10 MiB (CDC_MAX_BLOCK_SIZE) and 12 MiB leaves headroom for the AES padding of
/// encrypted data. Shared by sync put_block and the mobile upload-blks-api.
pub const MAX_BLOCK_UPLOAD_BYTES: usize = 12 * 1024 * 1024;

/// Upper bound on dirent names accepted in one request.
///
/// Endpoints take a name list and then do per-name work over the tree (a recursive walk
/// for `zip-task`, a linear scan for the batch ones), and a 64 MiB body holds well over
/// a million short names — hence a bound independent of the body limit.
pub const MAX_REQUEST_DIRENTS: usize = 1000;

/// Validate and normalise a client-supplied list of dirent names: cap the count and
/// dedupe in place, because every operation is "for each name, find the entry and add it
/// to the result" and a repeated name would be applied N times (e.g. the same entry
/// written into a directory object many times).
pub fn sanitize_dirent_list(names: &mut Vec<String>) -> Result<(), AppError> {
    if names.len() > MAX_REQUEST_DIRENTS {
        return Err(AppError::BadRequest(format!(
            "too many dirents: {} (max {MAX_REQUEST_DIRENTS})",
            names.len()
        )));
    }
    let mut seen = std::collections::HashSet::with_capacity(names.len());
    names.retain(|n| seen.insert(n.clone()));
    Ok(())
}

/// Read a request body with a hard size cap; oversized bodies map to 413.
pub async fn read_body_limited(
    body: axum::body::Body,
    limit: usize,
) -> Result<bytes::Bytes, AppError> {
    axum::body::to_bytes(body, limit).await.map_err(|e| {
        let too_large = std::error::Error::source(&e)
            .is_some_and(|src| src.is::<http_body_util::LengthLimitError>());
        if too_large {
            AppError::ContentTooLarge
        } else {
            AppError::Internal(e.to_string())
        }
    })
}

/// Stream a multipart field into memory under `limit`: reject an oversized part from
/// its Content-Length before buffering and cap the bytes read, so a lying header
/// cannot force a large buffer.
pub async fn read_multipart_field_limited(
    field: &mut axum::extract::multipart::Field<'_>,
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
    while let Some(c) = field
        .chunk()
        .await
        .map_err(|e| AppError::Internal(format!("multipart field read error: {e}")))?
    {
        if out.len() + c.len() > limit {
            return Err(AppError::ContentTooLarge);
        }
        out.extend_from_slice(&c);
    }
    Ok(out)
}

pub mod account;
pub mod activities;
pub mod api_key;
pub mod async_batch;
pub mod avatar;
pub mod batch;
pub mod chunked_upload;
pub mod client_login;
pub mod credential;
pub mod device_wipe;
pub mod devices;
pub mod dir;
pub mod exif;
pub mod file;
pub mod fileops;
pub mod groups;
pub mod history;
pub mod invitations;
pub mod links;
pub mod login;
pub mod metadata;
pub mod notifications;
pub mod password;
pub mod reindex;
pub mod repos;
pub mod search;
pub mod server_info;
pub mod share;
pub mod smart_link;
pub mod sso;
pub mod starred;
pub mod sync;
pub mod thumbnail;
pub mod trash;
pub mod upload_link;
pub mod user_avatar;
pub mod users;
pub mod web;
