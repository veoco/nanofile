use axum::{
    Json, Router,
    extract::{Path, State},
    http::{StatusCode, header},
    response::{IntoResponse, Response},
};
use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::path::PathBuf;
use std::sync::Arc;

use crate::AppState;
use crate::auth::middleware::AuthUser;
use crate::entity::avatar as avatar_entity;
use crate::error::AppError;

/// Default avatar SVG embedded at compile time.
static DEFAULT_AVATAR: &[u8] = include_bytes!("../../static/img/default-avatar.svg");

const DEFAULT_AVATAR_MIME: &str = "image/svg+xml";

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

    let avatar = avatar_entity::Entity::find()
        .filter(avatar_entity::Column::Email.eq(&email))
        .one(state.db.as_ref())
        .await?;

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

    let avatar = avatar_entity::Entity::find()
        .filter(avatar_entity::Column::Email.eq(&email))
        .one(state.db.as_ref())
        .await;

    match avatar {
        Ok(Some(a)) => serve_existing_avatar(&state, &a, size).await,
        Ok(None) | Err(_) => serve_default_avatar(),
    }
}

/// Serve a previously-uploaded avatar from disk, generating a thumbnail at the
/// requested `size` if one does not already exist.
async fn serve_existing_avatar(
    _state: &Arc<AppState>,
    avatar: &avatar_entity::Model,
    size: u32,
) -> Response {
    let storage_dir = avatar_storage_dir(&avatar.email);
    let thumbnail_path = storage_dir.join(format!("{}.png", size));

    // Fast path — thumbnail already cached on disk
    if thumbnail_path.exists() {
        match tokio::fs::read(&thumbnail_path).await {
            Ok(data) => {
                return (
                    StatusCode::OK,
                    [
                        (header::CONTENT_TYPE, "image/png"),
                        (header::CACHE_CONTROL, "public, max-age=86400"),
                    ],
                    data,
                )
                    .into_response();
            }
            Err(_) => return serve_default_avatar(),
        }
    }

    // Thumbnail miss — load the original and generate one
    let original_path = find_original_path(&storage_dir);
    let content = match original_path.and_then(|p| std::fs::read(p).ok()) {
        Some(c) => c,
        None => return serve_default_avatar(),
    };

    match generate_thumbnail(&content, size) {
        Ok(thumbnail_data) => {
            // Persist for future requests (non-fatal if it fails)
            let _ = tokio::fs::create_dir_all(&storage_dir).await;
            let _ = tokio::fs::write(&thumbnail_path, &thumbnail_data).await;

            (
                StatusCode::OK,
                [
                    (header::CONTENT_TYPE, "image/png"),
                    (header::CACHE_CONTROL, "public, max-age=86400"),
                ],
                thumbnail_data,
            )
                .into_response()
        }
        Err(_) => {
            // Fall back to serving the original at its native size
            (
                StatusCode::OK,
                [
                    (header::CONTENT_TYPE, avatar.mime_type.as_str()),
                    (header::CACHE_CONTROL, "public, max-age=86400"),
                ],
                content,
            )
                .into_response()
        }
    }
}

// ─── Thumbnail generation ────────────────────────────────────────────────────

/// Generate a square thumbnail from raw image data using the `image` crate.
fn generate_thumbnail(content: &[u8], size: u32) -> Result<Vec<u8>, AppError> {
    let img = image::load_from_memory(content)
        .map_err(|_| AppError::NotFound("unable to decode image".into()))?;

    let thumbnail = img.thumbnail(size, size);
    let mut output = Vec::new();
    thumbnail
        .write_to(
            &mut std::io::Cursor::new(&mut output),
            image::ImageFormat::Png,
        )
        .map_err(|e| AppError::Internal(e.to_string()))?;

    Ok(output)
}

// ─── Default avatar ──────────────────────────────────────────────────────────

/// Return the embedded default-avatar SVG as an HTTP response.
fn serve_default_avatar() -> Response {
    (
        StatusCode::OK,
        [(header::CONTENT_TYPE, DEFAULT_AVATAR_MIME)],
        DEFAULT_AVATAR.to_vec(),
    )
        .into_response()
}

/// Get the default-avatar bytes (used by non-HTTP contexts / tests).
pub fn default_avatar_bytes() -> &'static [u8] {
    DEFAULT_AVATAR
}

// ─── Helpers ─────────────────────────────────────────────────────────────────

/// Parse the `{size}` URL segment — accept a number or the literal `"default"`.
fn resolve_size(size_str: &str) -> u32 {
    if size_str.eq_ignore_ascii_case("default") {
        256
    } else {
        size_str.parse::<u32>().unwrap_or(256)
    }
}

/// Compute the on-disk storage directory for a user's avatar files.
///
/// Uses the first 16 hex characters of the SHA-256 of the user's email as the
/// directory name, matching seafile's `AVATAR_HASH_USERDIRNAMES` behaviour.
pub fn avatar_storage_dir(email: &str) -> PathBuf {
    let hash = hex::encode(Sha256::digest(email.as_bytes()));
    PathBuf::from("data/avatars").join(&hash[..16])
}

/// Find the original uploaded file inside a storage directory by trying known
/// image extensions in order.
fn find_original_path(dir: &std::path::Path) -> Option<PathBuf> {
    for ext in &["png", "jpg", "jpeg", "gif"] {
        let path = dir.join(format!("original.{}", ext));
        if path.exists() {
            return Some(path);
        }
    }
    None
}

/// Public helper: build the avatar URL path used by the rest of the codebase
/// (activities, groups, share links, etc.).
pub fn primary_avatar_url(email: &str, size: u32) -> String {
    format!("/avatars/user/{}/resized/{}/", email, size)
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
