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

/// Whether `block_id` is a Seafile block id: 40 lowercase hex characters.
///
/// Delegates to the storage layer: the sync block PUT, the block-upload commit and
/// the store itself must agree on one rule, or an uppercase id is accepted here and
/// then looked up under a name the writer never created.
pub fn is_valid_block_id(block_id: &str) -> bool {
    infra::storage::is_valid_block_id(block_id)
}

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

/// Reject a cross-library batch request: the submitted task (or synchronous
/// operation) carries one repo id, so running it against the source library
/// with the destination's paths would act on the wrong library.
pub fn ensure_same_batch_repo(src: &str, dst: &str, operation: &str) -> Result<(), AppError> {
    if src != dst {
        return Err(AppError::BadRequest(format!(
            "cross-repo {operation} not supported"
        )));
    }
    Ok(())
}

/// Normalize a batch parent path with the field-specific message the clients
/// already see (`"Invalid source path: …"`, `"Invalid path: …"`).
pub fn normalize_batch_dir(path: &str, label: &str) -> Result<String, AppError> {
    base::sanitize::safe_normalize_path(path)
        .map_err(|e| AppError::BadRequest(format!("Invalid {label}: {e}")))
}

/// Whether a failed body read is the peer giving up on its upload.
///
/// hyper surfaces a connection that ends before the declared `Content-Length` as a
/// `hyper::Error` of kind `Io` whose cause is an `io::Error` with `UnexpectedEof`
/// (not as its `IncompleteMessage` kind), and `http_body_util::Limited` /
/// `axum::Error` each wrap the previous error. The whole source chain is therefore
/// walked, for both spellings, plus the reset/abort kinds a peer disconnect
/// produces. `is_canceled`/`is_closed` cover HTTP/2 and shutdown.
fn body_read_was_aborted(error: &axum::Error) -> bool {
    use std::io::ErrorKind;

    let mut current: Option<&(dyn std::error::Error + 'static)> = Some(error);
    while let Some(error) = current {
        if let Some(hyper_error) = error.downcast_ref::<hyper::Error>()
            && (hyper_error.is_incomplete_message()
                || hyper_error.is_canceled()
                || hyper_error.is_closed())
        {
            return true;
        }
        if let Some(io_error) = error.downcast_ref::<std::io::Error>()
            && matches!(
                io_error.kind(),
                ErrorKind::UnexpectedEof
                    | ErrorKind::ConnectionReset
                    | ErrorKind::ConnectionAborted
                    | ErrorKind::BrokenPipe
            )
        {
            return true;
        }
        current = error.source();
    }
    false
}

/// Map a failed request-body read.
///
/// A client that disconnects or times out mid-upload is not a server fault: it is
/// answered with a 400 ([`AppError::ClientClosed`]) so the request neither shows
/// up as a 5xx in the log nor trips the trace layer's failure line. Everything
/// else stays a 500.
pub fn body_read_error(error: axum::Error) -> AppError {
    if body_read_was_aborted(&error) {
        AppError::ClientClosed
    } else {
        AppError::Internal(error.to_string())
    }
}

/// Read a request body with a hard size cap; oversized bodies map to 413, and a
/// peer that stops sending maps to 400 via [`body_read_error`].
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
            body_read_error(e)
        }
    })
}

pub mod account;
pub mod activities;
pub mod api_key;
pub mod async_batch;
pub mod avatar;
pub mod batch;
pub mod block;
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
pub mod json_or_form;
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

#[cfg(test)]
mod tests {
    use super::*;

    /// The peer-abort branch: an `UnexpectedEof` anywhere in the chain means the
    /// client stopped sending, whatever hyper wrapped it in.
    #[test]
    fn a_peer_abort_is_recognized_and_an_ordinary_failure_is_not() {
        let aborted = axum::Error::new(std::io::Error::from(std::io::ErrorKind::UnexpectedEof));
        assert!(body_read_was_aborted(&aborted));

        let reset = axum::Error::new(std::io::Error::from(std::io::ErrorKind::ConnectionReset));
        assert!(body_read_was_aborted(&reset));

        let other = axum::Error::new(std::io::Error::other("disk on fire"));
        assert!(!body_read_was_aborted(&other));
    }

    #[tokio::test]
    async fn an_aborted_upload_is_a_client_error_not_a_500() {
        let body = axum::body::Body::from_stream(futures::stream::once(async {
            Err::<bytes::Bytes, _>(std::io::Error::from(std::io::ErrorKind::UnexpectedEof))
        }));
        let error = read_body_limited(body, 1024).await.unwrap_err();
        assert!(matches!(error, AppError::ClientClosed), "got {error:?}");
    }

    #[tokio::test]
    async fn an_oversized_body_is_413() {
        let body = axum::body::Body::from(vec![0u8; 64]);
        let error = read_body_limited(body, 8).await.unwrap_err();
        assert!(matches!(error, AppError::ContentTooLarge), "got {error:?}");
    }

    /// The predicate both the sync PUT and the block-upload commit share: only a
    /// 40-character lowercase SHA-1 is an id the content-addressed store can serve.
    #[test]
    fn a_block_id_is_exactly_40_lowercase_hex_chars() {
        assert!(is_valid_block_id(
            "0123456789abcdef0123456789abcdef01234567"
        ));
        assert!(!is_valid_block_id(&"a".repeat(39)));
        assert!(!is_valid_block_id(&"a".repeat(41)));
        // The old validators each missed one side: `is_ascii_lowercase()` let
        // `g`-`z` through, `is_ascii_hexdigit()` let uppercase through. The store
        // only names blocks by the lowercase sha1 the writer computed.
        assert!(!is_valid_block_id(&"A".repeat(40)));
        assert!(!is_valid_block_id(&"g".repeat(40)));
        assert!(!is_valid_block_id(""));
    }
}
