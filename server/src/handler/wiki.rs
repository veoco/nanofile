use axum::{
    Json,
    extract::{Path, State},
};
use serde::Deserialize;
use std::sync::Arc;

use crate::AppState;
use crate::middleware::auth::AuthUser;
use crate::service::sharing::wiki;
use base::error::AppError;

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
    let wikis = wiki::list_wikis(&state.repos, _auth.user_id).await?;
    Ok(Json(serde_json::json!({"wikis": wikis})))
}

/// Fetch the wiki and verify the caller owns it.
async fn ensure_wiki_owner(state: &AppState, wiki_id: i32, user_id: i32) -> Result<(), AppError> {
    let wiki = state
        .repos
        .wiki
        .find_by_id(wiki_id)
        .await?
        .ok_or_else(|| AppError::NotFound("wiki not found".into()))?;
    if wiki.owner_id != user_id {
        return Err(AppError::Forbidden);
    }
    Ok(())
}

/// PUT /api/v2.1/wiki2/{wiki_id}/
pub async fn rename_wiki(
    auth: AuthUser,
    State(state): State<Arc<AppState>>,
    Path(wiki_id): Path<i32>,
    Json(req): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, AppError> {
    ensure_wiki_owner(&state, wiki_id, auth.user_id).await?;

    let name = req["name"]
        .as_str()
        .ok_or_else(|| AppError::BadRequest("name required".into()))?;

    wiki::rename_wiki(&state.repos, wiki_id, name).await?;

    Ok(Json(serde_json::json!({"success": true})))
}

/// POST /api/v2.1/wiki2/{wiki_id}/publish/
pub async fn publish_wiki(
    auth: AuthUser,
    State(state): State<Arc<AppState>>,
    Path(wiki_id): Path<i32>,
) -> Result<Json<serde_json::Value>, AppError> {
    ensure_wiki_owner(&state, wiki_id, auth.user_id).await?;
    wiki::publish_wiki(&state.repos, wiki_id).await?;
    Ok(Json(serde_json::json!({"success": true})))
}

/// DELETE /api/v2.1/wiki2/{wiki_id}/publish/
pub async fn unpublish_wiki(
    auth: AuthUser,
    State(state): State<Arc<AppState>>,
    Path(wiki_id): Path<i32>,
) -> Result<Json<serde_json::Value>, AppError> {
    ensure_wiki_owner(&state, wiki_id, auth.user_id).await?;
    wiki::unpublish_wiki(&state.repos, wiki_id).await?;
    Ok(Json(serde_json::json!({"success": true})))
}

/// DELETE /api/v2.1/wiki2/{wiki_id}/
pub async fn delete_wiki(
    auth: AuthUser,
    State(state): State<Arc<AppState>>,
    Path(wiki_id): Path<i32>,
) -> Result<Json<serde_json::Value>, AppError> {
    ensure_wiki_owner(&state, wiki_id, auth.user_id).await?;
    wiki::delete_wiki(&state.repos, wiki_id).await?;
    Ok(Json(serde_json::json!({"success": true})))
}
