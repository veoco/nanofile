use axum::Router;
use axum::routing::{get, put};
use axum::{
    Json,
    extract::{Path, State},
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

use crate::AppState;
use crate::auth::middleware::AuthUser;
use crate::error::AppError;

#[derive(Deserialize)]
pub struct CommentRequest {
    pub content: Option<String>,
}

#[derive(Serialize)]
pub struct CommentResponse {
    pub id: i32,
    pub content: String,
    pub resolved: Option<bool>,
    pub created_at: i64,
    pub user_email: String,
}

/// GET /api/v1/docs/{uuid}/comment/
pub async fn list_comments(
    _auth: AuthUser,
    State(state): State<Arc<AppState>>,
    Path(doc_uuid): Path<String>,
) -> Result<Json<Vec<CommentResponse>>, AppError> {
    let comments = state.repos.sdoc_comment.find_by_doc_uuid(&doc_uuid).await?;

    let mut result = Vec::new();
    for c in comments {
        let user = state.repos.user.find_by_id(c.user_id).await?;
        result.push(CommentResponse {
            id: c.id,
            content: c.content,
            resolved: c.resolved,
            created_at: c.created_at,
            user_email: user.map(|u| u.email).unwrap_or_default(),
        });
    }
    Ok(Json(result))
}

/// POST /api/v1/docs/{uuid}/comment/
pub async fn create_comment(
    _auth: AuthUser,
    State(state): State<Arc<AppState>>,
    Path(doc_uuid): Path<String>,
    Json(req): Json<CommentRequest>,
) -> Result<(axum::http::StatusCode, Json<CommentResponse>), AppError> {
    let content = req
        .content
        .ok_or_else(|| AppError::BadRequest("content required".into()))?;

    let inserted = state
        .repos
        .sdoc_comment
        .create(&doc_uuid, _auth.user_id, &content)
        .await?;

    let user = state.repos.user.find_by_id(_auth.user_id).await?;

    Ok((
        axum::http::StatusCode::CREATED,
        Json(CommentResponse {
            id: inserted.id,
            content: inserted.content,
            resolved: inserted.resolved,
            created_at: inserted.created_at,
            user_email: user.map(|u| u.email).unwrap_or_default(),
        }),
    ))
}

/// PUT /api/v1/docs/{uuid}/comment/{id}/
pub async fn resolve_comment(
    _auth: AuthUser,
    State(state): State<Arc<AppState>>,
    Path((_doc_uuid, comment_id)): Path<(String, i32)>,
    Json(req): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, AppError> {
    let resolved = req.get("resolved").and_then(|v| v.as_bool());
    if let Some(r) = resolved {
        state
            .repos
            .sdoc_comment
            .update_resolved(comment_id, r)
            .await?;
    }

    Ok(Json(serde_json::json!({"success": true})))
}

/// DELETE /api/v1/docs/{uuid}/comment/{id}/
pub async fn delete_comment(
    _auth: AuthUser,
    State(state): State<Arc<AppState>>,
    Path((_doc_uuid, comment_id)): Path<(String, i32)>,
) -> Result<Json<serde_json::Value>, AppError> {
    state.repos.sdoc_comment.delete_by_id(comment_id).await?;
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
