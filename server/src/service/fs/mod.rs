pub mod dir;
pub mod exif;
pub mod file;
pub mod fileops;
pub(crate) mod index_sync;
pub mod metadata;
pub mod quota;
pub mod search;
pub mod starred;
pub mod thumbnail;

/// Error for a failed parent-directory resolution on a write path.
///
/// A missing or non-directory component is the request's fault and must stay a
/// 404; anything else, a database failure say, is reported as internal.
pub(crate) fn resolve_parent_error(
    e: base::error::AppError,
    context: &str,
) -> base::error::AppError {
    use base::error::AppError;
    match e {
        AppError::NotFound(msg) => AppError::NotFound(msg),
        other => AppError::Internal(format!("{context}: {other}")),
    }
}
