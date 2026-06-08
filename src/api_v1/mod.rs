use axum::Router;
use axum::routing::{get, put};
use axum::{
    Json,
    extract::{Path, State},
};
use sea_orm::{ActiveModelTrait, ColumnTrait, EntityTrait, QueryFilter, Set};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

use crate::AppState;
use crate::auth::middleware::AuthUser;
use crate::entity::sdoc_comment;
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
    let comments = sdoc_comment::Entity::find()
        .filter(sdoc_comment::Column::DocUuid.eq(&doc_uuid))
        .all(state.db.as_ref())
        .await?;

    let mut result = Vec::new();
    for c in comments {
        let user = crate::entity::user::Entity::find_by_id(c.user_id)
            .one(state.db.as_ref())
            .await?;
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
    let now = chrono::Utc::now().timestamp();

    sdoc_comment::Entity::insert(sdoc_comment::ActiveModel {
        id: sea_orm::NotSet,
        doc_uuid: Set(doc_uuid.clone()),
        user_id: Set(_auth.user_id),
        content: Set(content.clone()),
        resolved: Set(Some(false)),
        created_at: Set(now),
    })
    .exec(state.db.as_ref())
    .await?;

    let inserted = sdoc_comment::Entity::find()
        .filter(sdoc_comment::Column::DocUuid.eq(&doc_uuid))
        .filter(sdoc_comment::Column::CreatedAt.eq(now))
        .one(state.db.as_ref())
        .await?
        .ok_or_else(|| AppError::Internal("comment not found after insert".into()))?;

    let user = crate::entity::user::Entity::find_by_id(_auth.user_id)
        .one(state.db.as_ref())
        .await?;

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
    let comment = sdoc_comment::Entity::find_by_id(comment_id)
        .one(state.db.as_ref())
        .await?
        .ok_or_else(|| AppError::NotFound("comment not found".into()))?;

    let resolved = req.get("resolved").and_then(|v| v.as_bool());
    let mut active: sdoc_comment::ActiveModel = comment.into();
    if let Some(r) = resolved {
        active.resolved = Set(Some(r));
    }
    active.update(state.db.as_ref()).await?;

    Ok(Json(serde_json::json!({"success": true})))
}

/// DELETE /api/v1/docs/{uuid}/comment/{id}/
pub async fn delete_comment(
    _auth: AuthUser,
    State(state): State<Arc<AppState>>,
    Path((_doc_uuid, comment_id)): Path<(String, i32)>,
) -> Result<Json<serde_json::Value>, AppError> {
    sdoc_comment::Entity::delete_by_id(comment_id)
        .exec(state.db.as_ref())
        .await?;
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

pub fn api_v1_routes() -> Router<Arc<AppState>> {
    Router::new().nest("/api/v1/docs/{doc_uuid}", doc_routes())
}
