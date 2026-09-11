//! App-wide HTTP request body limits.
//!
//! The JSON/Form default is deliberately small so a single request cannot make
//! a handler buffer gigabytes into memory. Upload-capable routes raise their
//! own `DefaultBodyLimit` to `max_upload_size_mb` (the raw body-stream limit,
//! `RequestBodyLimitLayer`, stays large for handlers that stream bodies).
//!
//! Limits are set once at startup from `[server]`.

use std::sync::atomic::{AtomicUsize, Ordering};

use axum::extract::DefaultBodyLimit;

static UPLOAD_LIMIT: AtomicUsize = AtomicUsize::new(4096 * 1024 * 1024);

/// Configure the upload body limit (called once at startup).
pub fn configure(max_upload_size_mb: u64) {
    UPLOAD_LIMIT.store(
        (max_upload_size_mb as usize).saturating_mul(1024 * 1024),
        Ordering::Relaxed,
    );
}

/// Current upload body limit in bytes.
pub fn upload_limit() -> usize {
    UPLOAD_LIMIT.load(Ordering::Relaxed)
}

/// `DefaultBodyLimit` layer for upload-capable routes.
pub fn upload_default_body_limit() -> DefaultBodyLimit {
    DefaultBodyLimit::max(upload_limit())
}
