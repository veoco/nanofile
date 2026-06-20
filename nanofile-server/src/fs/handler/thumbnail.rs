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
use crate::fs::service::thumbnail_service::ThumbnailService;

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

    let svc = ThumbnailService::new(
        state.repos.clone(),
        state.db.clone(),
        state.block_store.clone(),
        state.block_dir.clone(),
    );
    let data = svc.get_thumbnail(&repo_id, path, size).await?;

    Ok((StatusCode::OK, [(header::CONTENT_TYPE, "image/png")], data).into_response())
}

pub fn thumbnail_routes() -> Router<Arc<AppState>> {
    Router::new().route("/{repo_id}/thumbnail/", axum::routing::get(get_thumbnail))
}
