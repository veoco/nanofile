//! App-wide HTTP request body limits: the JSON/Form default is deliberately
//! small so one request cannot buffer gigabytes, and upload-capable routes raise
//! their own `DefaultBodyLimit`, while the raw body-stream limit
//! (`RequestBodyLimitLayer`) stays large for handlers that stream bodies.

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
