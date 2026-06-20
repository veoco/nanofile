use axum::{
    Json, Router,
    body::Bytes,
    extract::{Query, State},
    http::HeaderMap,
};
use serde::Deserialize;
use std::sync::Arc;

use crate::AppState;
use crate::auth::middleware::AuthUser;
use crate::error::AppError;
use crate::fs::service::starred::StarredService;

pub use crate::fs::service::starred::StarredFileEntry;

pub async fn list_starred_files(
    auth: AuthUser,
    State(state): State<Arc<AppState>>,
) -> Result<Json<Vec<StarredFileEntry>>, AppError> {
    let svc = StarredService::new(state.repos.clone(), state.db.clone());
    let result = svc.list_starred_files(auth.user_id).await?;
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
    let svc = StarredService::new(state.repos.clone(), state.db.clone());
    let result = svc.get_starred_items(auth.user_id, &auth.email).await?;
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
            repo_id: crate::common::util::extract_multipart_field(&bytes, "repo_id")
                .ok_or_else(|| AppError::BadRequest("repo_id required".into()))?,
            path: crate::common::util::extract_multipart_field(&bytes, "path")
                .ok_or_else(|| AppError::BadRequest("path required".into()))?,
        }
    };

    // Permission check
    crate::storage::check_repo_read_permission(state.db.as_ref(), &req.repo_id, auth.user_id)
        .await?;

    let svc = StarredService::new(state.repos.clone(), state.db.clone());
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
    let svc = StarredService::new(state.repos.clone(), state.db.clone());
    svc.unstar_item(auth.user_id, &query.repo_id, &query.path)
        .await?;

    Ok(Json(serde_json::json!({"success": true})))
}
