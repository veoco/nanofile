use axum::{
    Json,
    extract::{Path, State},
};
use sea_orm::{ActiveModelTrait, ColumnTrait, EntityTrait, QueryFilter, Set};
use serde::Deserialize;
use std::sync::Arc;

use crate::AppState;
use crate::auth::middleware::AuthUser;
use crate::entity::wiki;
use crate::error::AppError;

#[derive(Deserialize)]
pub struct WikiCreateRequest {
    pub repo_id: String,
    pub name: String,
    pub permission: Option<String>,
}

/// GET /api/v2.1/wikis/
pub async fn list_wikis(
    _auth: AuthUser,
    State(state): State<Arc<AppState>>,
) -> Result<Json<serde_json::Value>, AppError> {
    let wikis = wiki::Entity::find()
        .filter(wiki::Column::OwnerId.eq(_auth.user_id))
        .all(state.db.as_ref())
        .await?;

    Ok(Json(serde_json::json!({"wikis": wikis})))
}

/// PUT /api/v2.1/wiki2/{wiki_id}/
pub async fn rename_wiki(
    _auth: AuthUser,
    State(state): State<Arc<AppState>>,
    Path(wiki_id): Path<i32>,
    Json(req): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, AppError> {
    let name = req["name"]
        .as_str()
        .ok_or_else(|| AppError::BadRequest("name required".into()))?;

    let w = wiki::Entity::find_by_id(wiki_id)
        .one(state.db.as_ref())
        .await?
        .ok_or_else(|| AppError::NotFound("wiki not found".into()))?;

    let mut active: wiki::ActiveModel = w.into();
    active.name = Set(name.to_string());
    active.update(state.db.as_ref()).await?;

    Ok(Json(serde_json::json!({"success": true})))
}

/// POST /api/v2.1/wiki2/{wiki_id}/publish/
pub async fn publish_wiki(
    _auth: AuthUser,
    State(state): State<Arc<AppState>>,
    Path(wiki_id): Path<i32>,
) -> Result<Json<serde_json::Value>, AppError> {
    let w = wiki::Entity::find_by_id(wiki_id)
        .one(state.db.as_ref())
        .await?
        .ok_or_else(|| AppError::NotFound("wiki not found".into()))?;

    let mut active: wiki::ActiveModel = w.into();
    active.published = Set(Some(true));
    active.update(state.db.as_ref()).await?;

    Ok(Json(serde_json::json!({"success": true})))
}

/// DELETE /api/v2.1/wiki2/{wiki_id}/publish/
pub async fn unpublish_wiki(
    _auth: AuthUser,
    State(state): State<Arc<AppState>>,
    Path(wiki_id): Path<i32>,
) -> Result<Json<serde_json::Value>, AppError> {
    let w = wiki::Entity::find_by_id(wiki_id)
        .one(state.db.as_ref())
        .await?
        .ok_or_else(|| AppError::NotFound("wiki not found".into()))?;

    let mut active: wiki::ActiveModel = w.into();
    active.published = Set(Some(false));
    active.update(state.db.as_ref()).await?;

    Ok(Json(serde_json::json!({"success": true})))
}

/// DELETE /api/v2.1/wiki2/{wiki_id}/
pub async fn delete_wiki(
    _auth: AuthUser,
    State(state): State<Arc<AppState>>,
    Path(wiki_id): Path<i32>,
) -> Result<Json<serde_json::Value>, AppError> {
    wiki::Entity::delete_by_id(wiki_id)
        .exec(state.db.as_ref())
        .await?;
    Ok(Json(serde_json::json!({"success": true})))
}
