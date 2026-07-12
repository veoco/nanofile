use axum::Router;
use axum::routing::{get, put};
use axum::{
    Json,
    extract::{Path, State},
};
use serde::Deserialize;
use std::sync::Arc;

use crate::AppState;
use crate::middleware::auth::AuthUser;
use base::error::AppError;

#[derive(Deserialize)]
pub struct CommentRequest {
    pub content: Option<String>,
}

/// GET /api/v1/docs/{uuid}/comment/
pub async fn list_comments(
    _auth: AuthUser,
    State(state): State<Arc<AppState>>,
    Path(doc_uuid): Path<String>,
) -> Result<Json<Vec<crate::service::sdoc::CommentResponse>>, AppError> {
    let comments = state.sdoc_service().list_comments(&doc_uuid).await?;
    Ok(Json(comments))
}

/// POST /api/v1/docs/{uuid}/comment/
pub async fn create_comment(
    _auth: AuthUser,
    State(state): State<Arc<AppState>>,
    Path(doc_uuid): Path<String>,
    Json(req): Json<CommentRequest>,
) -> Result<
    (
        axum::http::StatusCode,
        Json<crate::service::sdoc::CommentResponse>,
    ),
    AppError,
> {
    let content = req
        .content
        .ok_or_else(|| AppError::BadRequest("content required".into()))?;

    let comment = state
        .sdoc_service()
        .create_comment(&doc_uuid, _auth.user_id, &content)
        .await?;

    Ok((axum::http::StatusCode::CREATED, Json(comment)))
}

/// PUT /api/v1/docs/{uuid}/comment/{id}/
pub async fn resolve_comment(
    _auth: AuthUser,
    State(state): State<Arc<AppState>>,
    Path((_doc_uuid, comment_id)): Path<(String, i32)>,
    Json(req): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, AppError> {
    if let Some(r) = req.get("resolved").and_then(|v| v.as_bool()) {
        state.sdoc_service().resolve_comment(comment_id, r).await?;
    }
    Ok(Json(serde_json::json!({"success": true})))
}

/// DELETE /api/v1/docs/{uuid}/comment/{id}/
pub async fn delete_comment(
    _auth: AuthUser,
    State(state): State<Arc<AppState>>,
    Path((_doc_uuid, comment_id)): Path<(String, i32)>,
) -> Result<Json<serde_json::Value>, AppError> {
    state.sdoc_service().delete_comment(comment_id).await?;
    Ok(Json(serde_json::json!({"success": true})))
}

fn doc_routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/comment/", get(list_comments).post(create_comment))
        .route(
            "/comment/{comment_id}/",
            put(resolve_comment).delete(delete_comment),
        )
}

pub fn sdoc_routes() -> Router<Arc<AppState>> {
    Router::new().nest("/api/v1/docs/{doc_uuid}", doc_routes())
}
