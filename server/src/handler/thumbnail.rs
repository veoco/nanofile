use axum::{
    Router,
    extract::{Query, State},
    http::{StatusCode, header},
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
    let data = svc.get_thumbnail(&repo_id, &path, size).await?;

    Ok((
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, "image/png"),
            // Matching seahub's THUMBNAIL_CACHE_DAYS=7 → 604800 seconds
            (header::CACHE_CONTROL, "private, max-age=604800"),
        ],
        data,
    )
        .into_response())
}

pub fn thumbnail_routes() -> Router<Arc<AppState>> {
    Router::new().route("/{repo_id}/thumbnail/", axum::routing::get(get_thumbnail))
}
