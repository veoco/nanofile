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
/// A missing component, or a component that is not a directory (a path "inside"
/// a file), is the request's fault and must stay a 404; wrapping it into
/// `Internal` would answer 500 for a client mistake. Anything else — a database
/// failure, say — keeps its context and is reported as internal.
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
