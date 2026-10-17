//! WebDAV protocol endpoint (`/dav/{repo_id}/...`).
//!
//! Hand-written RFC 4918 implementation over the existing content-addressed FS
//! services; auth is HTTP Basic with per-library WebDAV keys (see `auth`).

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
/// The wildcard route does not match the bare root (`{*path}` needs a leading
/// `/`), so the root and its trailing-slash variant are registered separately.
pub fn webdav_routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/dav/{repo_id}", any(handlers::dispatch_root))
        .route("/dav/{repo_id}/", any(handlers::dispatch_root))
        .route("/dav/{repo_id}/{*path}", any(handlers::dispatch_path))
}
