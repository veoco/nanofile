use axum::{
    Json, Router,
    body::Bytes,
    extract::{Query, State},
    http::HeaderMap,
};
use serde::Deserialize;
use std::sync::Arc;

use crate::AppState;
use crate::handler::ok_json;
use crate::middleware::auth::AuthUser;
use base::error::AppError;

pub use crate::service::fs::starred::StarredFileEntry;

pub async fn list_starred_files(
    auth: AuthUser,
    State(state): State<Arc<AppState>>,
) -> Result<Json<Vec<StarredFileEntry>>, AppError> {
    let svc = state.starred_service();
    let result = svc
        .list_starred_files(auth.user_id, &auth.repo_scope())
        .await?;
    Ok(Json(result))
}

pub fn starred_routes() -> Router<Arc<AppState>> {
    Router::new().route("/starredfiles/", axum::routing::get(list_starred_files))
}

#[derive(Deserialize)]
pub struct StarOrUnstarRequest {
    pub repo_id: String,
    pub path: String,
}

#[derive(Deserialize)]
pub struct UnstarQuery {
    pub repo_id: String,
    pub path: String,
}

pub async fn get_starred_items(
    auth: AuthUser,
    State(state): State<Arc<AppState>>,
) -> Result<Json<serde_json::Value>, AppError> {
    let svc = state.starred_service();
    let result = svc
        .get_starred_items(auth.user_id, &auth.email, &auth.repo_scope())
        .await?;
    Ok(Json(result))
}

pub async fn star_item(
    auth: AuthUser,
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    bytes: Bytes,
) -> Result<Json<serde_json::Value>, AppError> {
    // Parse request (JSON or multipart)
    let req = if headers
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .is_some_and(|ct| ct.contains("json"))
    {
        serde_json::from_slice::<StarOrUnstarRequest>(&bytes)?
    } else {
        StarOrUnstarRequest {
            repo_id: infra::common::util::extract_multipart_field(&bytes, "repo_id")
                .ok_or_else(|| AppError::BadRequest("repo_id required".into()))?,
            path: infra::common::util::extract_multipart_field(&bytes, "path")
                .ok_or_else(|| AppError::BadRequest("path required".into()))?,
        }
    };

    // A key's library scope is a ceiling, and the repo arrives in the body
    // rather than the URI, so the route-table guard never sees it.
    auth.require_repo_read(&state, &req.repo_id).await?;

    let svc = state.starred_service();
    let result = svc
        .star_item(auth.user_id, &auth.email, &req.repo_id, &req.path)
        .await?;

    Ok(Json(result))
}

pub async fn unstar_item(
    auth: AuthUser,
    State(state): State<Arc<AppState>>,
    Query(query): Query<UnstarQuery>,
) -> Result<Json<serde_json::Value>, AppError> {
    // Permission check (matches star_item)
    auth.require_repo_read(&state, &query.repo_id).await?;

    let svc = state.starred_service();
    svc.unstar_item(auth.user_id, &query.repo_id, &query.path)
        .await?;

    Ok(ok_json())
}
