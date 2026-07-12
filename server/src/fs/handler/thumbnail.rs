use axum::{
    Router,
    extract::{Path, Query, State},
    http::{StatusCode, header},
    response::{IntoResponse, Response},
};
use serde::Deserialize;
use std::sync::Arc;

use crate::AppState;
use crate::auth::middleware::AuthUser;
use crate::error::AppError;

#[derive(Deserialize)]
pub struct ThumbnailQuery {
    pub p: Option<String>,
    pub size: Option<u32>,
}

pub async fn get_thumbnail(
    _auth: AuthUser,
    State(state): State<Arc<AppState>>,
    Path(repo_id): Path<String>,
    Query(query): Query<ThumbnailQuery>,
) -> Result<Response, AppError> {
    let path = query
        .p
        .as_deref()
        .ok_or_else(|| AppError::BadRequest("path required".into()))?;
    let size = query.size.unwrap_or(48);

    let svc = state.thumbnail_service();
    let data = svc.get_thumbnail(&repo_id, path, size).await?;

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
