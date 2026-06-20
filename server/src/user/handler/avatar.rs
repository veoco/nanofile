use axum::{
    Json, Router,
    extract::{Path, State},
    http::{StatusCode, header},
    response::{IntoResponse, Response},
};
use serde::Serialize;
use std::sync::Arc;

use crate::AppState;
use crate::auth::middleware::AuthUser;
use crate::error::AppError;
use crate::user::service::AvatarService;
use crate::user::service::{primary_avatar_url, resolve_size};

// ─── API response types ──────────────────────────────────────────────────────

#[derive(Serialize)]
pub struct AvatarResponse {
    pub url: String,
    pub is_default: bool,
    pub mtime: i64,
}

// ─── JSON API endpoint (seafile API2) ───────────────────────────────────────

/// `GET /api2/avatars/user/{email}/resized/{size}/`
///
/// Returns avatar metadata as JSON: the URL to fetch the actual image,
/// whether it's the default, and the last-modified timestamp.
pub async fn get_avatar(
    _auth: AuthUser,
    State(state): State<Arc<AppState>>,
    Path((email, size_str)): Path<(String, String)>,
) -> Result<Json<AvatarResponse>, AppError> {
    // Seahub compatibility: always return a URL (default if no avatar or user
    // doesn't exist).  Clients query avatars for any email without checking
    // user existence first.
    let size = resolve_size(&size_str);

    let svc = AvatarService::new(state.db.as_ref(), &state.repos);
    let avatar = svc.find_avatar(&email).await?;

    match avatar {
        Some(a) => Ok(Json(AvatarResponse {
            url: primary_avatar_url(&email, size),
            is_default: false,
            mtime: a.date_uploaded,
        })),
        None => Ok(Json(AvatarResponse {
            url: primary_avatar_url(&email, size),
            is_default: true,
            mtime: 0,
        })),
    }
}

// ─── Image serving endpoint (raw binary) ─────────────────────────────────────

/// `GET /avatars/user/{email}/resized/{size}/`
///
/// Serves the actual avatar image binary. Returns the default-avatar SVG when
/// the user has not uploaded an avatar. Generates and caches thumbnails at any
/// requested size on first access (thumbnail-on-demand).
pub async fn serve_avatar_image(
    State(state): State<Arc<AppState>>,
    Path((email, size_str)): Path<(String, String)>,
) -> Response {
    let size = resolve_size(&size_str);

    let svc = AvatarService::new(state.db.as_ref(), &state.repos);
    let avatar = svc.find_avatar(&email).await;

    match avatar {
        Ok(Some(a)) => match svc.read_avatar_bytes(&a, size).await {
            Some((data, mime)) => (
                StatusCode::OK,
                [
                    (header::CONTENT_TYPE, mime),
                    (header::CACHE_CONTROL, "public, max-age=86400"),
                ],
                data,
            )
                .into_response(),
            None => serve_default_avatar(),
        },
        Ok(None) | Err(_) => serve_default_avatar(),
    }
}

// ─── Default avatar ──────────────────────────────────────────────────────────

/// Return the embedded default-avatar SVG as an HTTP response.
fn serve_default_avatar() -> Response {
    (
        StatusCode::OK,
        [(header::CONTENT_TYPE, "image/svg+xml")],
        crate::user::service::default_avatar_bytes().to_vec(),
    )
        .into_response()
}

// ─── Routes ──────────────────────────────────────────────────────────────────

/// JSON-API route, nested under `/api2` by the caller.
pub fn api_routes() -> Router<Arc<AppState>> {
    Router::new().route(
        "/avatars/user/{email}/resized/{size}/",
        axum::routing::get(get_avatar),
    )
}

/// Image-binary route, mounted at the application root so the returned
/// `primary_avatar_url()` paths resolve directly.
pub fn image_routes() -> Router<Arc<AppState>> {
    Router::new().route(
        "/avatars/user/{email}/resized/{size}/",
        axum::routing::get(serve_avatar_image),
    )
}
