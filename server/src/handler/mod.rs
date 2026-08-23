//! HTTP handler layer — thin adapters between axum and the service layer.
//!
//! REST API handlers live as flat files under `handler/`, grouped by domain.
//! The sync protocol (`/seafhttp/`) and file serving (`/download-api/`, etc.)
//! each have their own subdirectories due to their distinct auth patterns.

use axum::Json;
use base::error::AppError;

/// Standard success response body: `{"success": true}`.
pub fn ok_json() -> Json<serde_json::Value> {
    Json(serde_json::json!({"success": true}))
}

/// Upper bound for pure-metadata request bodies (login, file/dir/repo/trash
/// ops). These never carry file bytes; the global upload limit is far larger.
pub const MAX_SMALL_BODY_BYTES: usize = 1024 * 1024;

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

pub mod account;
pub mod activities;
pub mod async_batch;
pub mod avatar;
pub mod batch;
pub mod chunked_upload;
pub mod client_login;
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
pub mod two_factor;
pub mod upload_link;
pub mod user_avatar;
pub mod users;
pub mod web;
pub mod webdav_key;
