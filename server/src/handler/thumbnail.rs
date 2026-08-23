use axum::{
    Router,
    extract::{Query, State},
    http::{HeaderMap, StatusCode, header},
    response::{IntoResponse, Response},
};
use serde::Deserialize;
use std::sync::Arc;

use crate::AppState;
use crate::middleware::repo_extractor::RepoPathRead;
use base::error::AppError;

#[derive(Deserialize)]
pub struct ThumbnailQuery {
    pub p: Option<String>,
    pub size: Option<u32>,
}

pub async fn get_thumbnail(
    path: RepoPathRead,
    State(state): State<Arc<AppState>>,
    Query(query): Query<ThumbnailQuery>,
    headers: HeaderMap,
) -> Result<Response, AppError> {
    let repo_id = path.repo_id;
    let path = query
        .p
        .as_deref()
        .ok_or_else(|| AppError::BadRequest("path required".into()))?;
    let path = base::sanitize::safe_normalize_path(path)
        .map_err(|e| AppError::BadRequest(format!("Invalid path: {e}")))?;
    let size = query.size.unwrap_or(48);
    if size > crate::thumbnail_util::MAX_THUMBNAIL_SIZE {
        return Err(AppError::BadRequest(format!(
            "thumbnail size too large (max {})",
            crate::thumbnail_util::MAX_THUMBNAIL_SIZE
        )));
    }

    let svc = state.thumbnail_service();
    let (data, etag) = svc.get_thumbnail(&repo_id, &path, size).await?;

    // Conditional request: a matching validator short-circuits to 304 without
    // re-sending the thumbnail body (mirrors the download endpoint).
    let if_none_match = headers
        .get(header::IF_NONE_MATCH)
        .and_then(|v| v.to_str().ok());
    let matches = match if_none_match {
        Some("*") => true,
        Some(v) => v.split(',').any(|t| t.trim() == etag),
        None => false,
    };
    if matches {
        // 304 carries the same cache headers as the 200 so the client can
        // keep revalidating without a full refetch.
        return Ok((
            StatusCode::NOT_MODIFIED,
            [
                // Matching seahub's THUMBNAIL_CACHE_DAYS=7 → 604800 seconds
                (header::CACHE_CONTROL, "private, max-age=604800"),
                (header::ETAG, &etag),
            ],
        )
            .into_response());
    }

    Ok((
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, "image/png"),
            // Matching seahub's THUMBNAIL_CACHE_DAYS=7 → 604800 seconds
            (header::CACHE_CONTROL, "private, max-age=604800"),
            (header::ETAG, &etag),
        ],
        data,
    )
        .into_response())
}

pub fn thumbnail_routes() -> Router<Arc<AppState>> {
    Router::new().route("/{repo_id}/thumbnail/", axum::routing::get(get_thumbnail))
}
