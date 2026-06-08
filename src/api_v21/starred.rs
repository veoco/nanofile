use axum::Json;
use axum::extract::{Query, State};
use sea_orm::{ActiveModelTrait, ColumnTrait, EntityTrait, QueryFilter, Set};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

use crate::AppState;
use crate::auth::middleware::AuthUser;
use crate::entity::starred_file;
use crate::error::AppError;

#[derive(Serialize)]
pub struct StarredItemListResponse {
    pub starred_item_list: Vec<StarredItemResponse>,
}

#[derive(Serialize)]
pub struct StarredItemResponse {
    pub repo_id: String,
    pub path: String,
    pub repo_name: Option<String>,
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

/// `GET /api/v2.1/starred-items/`
pub async fn get_starred_items(
    auth: AuthUser,
    State(state): State<Arc<AppState>>,
) -> Result<Json<StarredItemListResponse>, AppError> {
    let entries = starred_file::Entity::find()
        .filter(starred_file::Column::UserId.eq(auth.user_id))
        .all(state.db.as_ref())
        .await?;

    let items: Vec<StarredItemResponse> = entries
        .into_iter()
        .map(|e| StarredItemResponse {
            repo_id: e.repo_id,
            path: e.path,
            repo_name: None,
        })
        .collect();

    Ok(Json(StarredItemListResponse {
        starred_item_list: items,
    }))
}

/// `POST /api/v2.1/starred-items/`
pub async fn star_item(
    auth: AuthUser,
    State(state): State<Arc<AppState>>,
    Json(req): Json<StarOrUnstarRequest>,
) -> Result<Json<serde_json::Value>, AppError> {
    let now = chrono::Utc::now().timestamp();

    starred_file::ActiveModel {
        id: sea_orm::NotSet,
        repo_id: Set(req.repo_id),
        path: Set(req.path),
        user_id: Set(auth.user_id),
        created_at: Set(now),
    }
    .insert(state.db.as_ref())
    .await?;

    Ok(Json(serde_json::json!({"success": true})))
}

/// `DELETE /api/v2.1/starred-items/`
///
/// Seafile clients send this as DELETE with query params: `?repo_id=X&path=Y`
pub async fn unstar_item(
    auth: AuthUser,
    State(state): State<Arc<AppState>>,
    Query(query): Query<UnstarQuery>,
) -> Result<Json<serde_json::Value>, AppError> {
    starred_file::Entity::delete_many()
        .filter(starred_file::Column::UserId.eq(auth.user_id))
        .filter(starred_file::Column::RepoId.eq(&query.repo_id))
        .filter(starred_file::Column::Path.eq(&query.path))
        .exec(state.db.as_ref())
        .await?;

    Ok(Json(serde_json::json!({"success": true})))
}
