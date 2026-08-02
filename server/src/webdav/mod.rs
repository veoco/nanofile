//! WebDAV protocol endpoint (`/dav/{repo_id}/...`).
//!
//! Hand-written WebDAV implementation mapping RFC 4918 methods onto the
//! existing content-addressed FS services. Authentication uses HTTP Basic
//! with per-library WebDAV keys (see `auth`).

pub mod auth;
pub mod handlers;
pub mod propfind;
pub mod util;
pub mod xml;

use axum::Router;
use axum::routing::any;
use std::sync::Arc;

use crate::AppState;

/// Build the WebDAV route tree mounted at `/dav/{repo_id}/...`.
///
/// Three routes are registered because the wildcard route does not match the
/// bare root (`{*path}` requires a leading `/`):
/// - `/dav/{repo_id}`
/// - `/dav/{repo_id}/` (trailing-slash variant — WebDAV clients commonly
///   request the root with a trailing slash)
/// - `/dav/{repo_id}/{*path}`
pub fn webdav_routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/dav/{repo_id}", any(handlers::dispatch_root))
        .route("/dav/{repo_id}/", any(handlers::dispatch_root))
        .route("/dav/{repo_id}/{*path}", any(handlers::dispatch_path))
}
