use axum::{
    Json, Router,
    extract::{Path, State},
    http::{StatusCode, header},
    response::{IntoResponse, Response},
};
use serde::Serialize;
use std::sync::Arc;

use crate::AppState;
use crate::middleware::auth::AuthUser;
use crate::service::user::{primary_avatar_url, resolve_size};
use base::error::AppError;

#[derive(Serialize)]
pub struct AvatarResponse {
    pub url: String,
    pub is_default: bool,
    pub mtime: i64,
}

/// Parse the `{size}` segment, reject absurd sizes (a `size² × 4` allocation → OOM)
/// and snap to [`crate::thumbnail_util::AVATAR_SIZES`]: this image route is
/// unauthenticated, so a distinct cache entry per integer would let a caller walk the
/// range and force a stored thumbnail for each.
fn parse_resize_size(size_str: &str) -> Result<u32, AppError> {
    let size = resolve_size(size_str);
    if size > crate::thumbnail_util::MAX_THUMBNAIL_SIZE {
        return Err(AppError::BadRequest(format!(
            "avatar size too large (max {})",
            crate::thumbnail_util::MAX_THUMBNAIL_SIZE
        )));
    }
    Ok(crate::thumbnail_util::canonical_avatar_size(size))
}

/// `GET /api2/avatars/user/{email}/resized/{size}/` — avatar metadata as JSON: image
/// URL, is_default, mtime.
pub async fn get_avatar(
    _auth: AuthUser,
    State(state): State<Arc<AppState>>,
    Path((email, size_str)): Path<(String, String)>,
) -> Result<Json<AvatarResponse>, AppError> {
    // Seahub compatibility: always return a URL (default if no avatar or user
    // doesn't exist); clients query any email without checking existence first.
    let size = parse_resize_size(&size_str)?;

    // The client uses `url` as the fetch target, so it must be absolute
    // (matching seahub's api_avatar_url behaviour).
    let origin = state.config().server.site_url_origin();
    let url = format!("{}{}", origin, primary_avatar_url(&email, size));

    let svc = state.avatar_service();
    let avatar = svc.find_avatar(&email).await?;

    match avatar {
        Some(a) => Ok(Json(AvatarResponse {
            url,
            is_default: false,
            mtime: a.date_uploaded,
        })),
        None => Ok(Json(AvatarResponse {
            url,
            is_default: true,
            mtime: 0,
        })),
    }
}

/// `GET /avatars/user/{email}/resized/{size}/` — the image binary or the default-avatar
/// SVG; thumbnails are generated and cached on first access.
pub async fn serve_avatar_image(
    State(state): State<Arc<AppState>>,
    Path((email, size_str)): Path<(String, String)>,
) -> Response {
    let size = match parse_resize_size(&size_str) {
        Ok(s) => s,
        Err(e) => return e.into_response(),
    };

    let svc = state.avatar_service();
    let avatar = svc.find_avatar(&email).await;

    match avatar {
        Ok(Some(a)) => match svc.read_avatar_bytes(&a, size).await {
            Some((data, mime)) => (
                StatusCode::OK,
                [
                    (header::CONTENT_TYPE, mime),
                    // `private`: the image identifies one account, so a shared
                    // cache must not store it (the URL is guessable from an
                    // email address).
                    (header::CACHE_CONTROL, "private, max-age=86400"),
                ],
                data,
            )
                .into_response(),
            None => serve_default_avatar(),
        },
        Ok(None) | Err(_) => serve_default_avatar(),
    }
}

fn serve_default_avatar() -> Response {
    (
        StatusCode::OK,
        [(header::CONTENT_TYPE, "image/svg+xml")],
        crate::service::user::default_avatar_bytes().to_vec(),
    )
        .into_response()
}

/// JSON-API route, nested under `/api2` by the caller.
pub fn api_routes() -> Router<Arc<AppState>> {
    Router::new().route(
        "/avatars/user/{email}/resized/{size}/",
        axum::routing::get(get_avatar),
    )
}

/// Image-binary route, mounted at the root so `primary_avatar_url()` paths resolve.
pub fn image_routes() -> Router<Arc<AppState>> {
    Router::new().route(
        "/avatars/user/{email}/resized/{size}/",
        axum::routing::get(serve_avatar_image),
    )
}
