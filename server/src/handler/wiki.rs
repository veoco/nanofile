use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use serde::Deserialize;
use std::sync::Arc;

use crate::AppState;
use crate::handler::ok_json;
use crate::middleware::auth::AuthUser;
use base::error::AppError;

/// Seafile wiki2 — a wiki is a library marked `type='wiki'`. `wiki_id` in the
/// routes is the 36-char library id (`repo_id`).

#[derive(Deserialize)]
pub struct CreateWikiRequest {
    pub name: String,
    #[serde(default)]
    pub owner: Option<String>,
}

#[derive(Deserialize)]
pub struct UpdateWikiRequest {
    pub wiki_name: Option<String>,
}

#[derive(Deserialize)]
pub struct PublishWikiRequest {
    pub publish_url: Option<String>,
    #[serde(default)]
    pub enable_server_render: Option<String>,
}

/// `GET /api/v2.1/wikis/` — legacy wiki1 list (empty; the new model has none).
pub async fn list_wikis_v1(
    _auth: AuthUser,
    State(state): State<Arc<AppState>>,
) -> Result<Json<serde_json::Value>, AppError> {
    Ok(Json(state.wiki_service().list_wikis_v1().await?))
}

/// `GET /api/v2.1/wikis2/` — list wikis (mine + shared).
pub async fn list_wikis_v2(
    auth: AuthUser,
    State(state): State<Arc<AppState>>,
) -> Result<Json<serde_json::Value>, AppError> {
    Ok(Json(
        state
            .wiki_service()
            .list_wikis_v2(auth.user_id, &auth.email)
            .await?,
    ))
}

/// `POST /api/v2.1/wikis2/` — create a wiki.
pub async fn create_wiki(
    auth: AuthUser,
    State(state): State<Arc<AppState>>,
    Json(req): Json<CreateWikiRequest>,
) -> Result<(StatusCode, Json<serde_json::Value>), AppError> {
    if req.owner.is_some() && req.owner.as_deref() != Some("me") {
        return Err(AppError::BadRequest(
            "group wikis are not supported yet".into(),
        ));
    }
    let wiki = state
        .wiki_service()
        .create_wiki(auth.user_id, &auth.email, &req.name)
        .await?;
    Ok((StatusCode::CREATED, Json(wiki)))
}

/// `PUT /api/v2.1/wiki2/{repo_id}/` — rename / set icon / color.
pub async fn rename_wiki(
    auth: AuthUser,
    State(state): State<Arc<AppState>>,
    Path(repo_id): Path<String>,
    Json(req): Json<UpdateWikiRequest>,
) -> Result<Json<serde_json::Value>, AppError> {
    if let Some(name) = req.wiki_name.as_deref() {
        state
            .wiki_service()
            .rename_wiki(&repo_id, auth.user_id, name)
            .await?;
    }
    Ok(ok_json())
}

/// `DELETE /api/v2.1/wiki2/{repo_id}/` — delete a wiki.
pub async fn delete_wiki(
    auth: AuthUser,
    State(state): State<Arc<AppState>>,
    Path(repo_id): Path<String>,
) -> Result<Json<serde_json::Value>, AppError> {
    state
        .wiki_service()
        .delete_wiki(&repo_id, auth.user_id)
        .await?;
    Ok(ok_json())
}

/// `GET /api/v2.1/wiki2/{repo_id}/publish/` — publish info.
pub async fn publish_info(
    auth: AuthUser,
    State(state): State<Arc<AppState>>,
    Path(repo_id): Path<String>,
) -> Result<Json<serde_json::Value>, AppError> {
    Ok(Json(
        state
            .wiki_service()
            .publish_info(&repo_id, auth.user_id)
            .await?,
    ))
}

/// `POST /api/v2.1/wiki2/{repo_id}/publish/` — publish a wiki.
pub async fn publish_wiki(
    auth: AuthUser,
    State(state): State<Arc<AppState>>,
    Path(repo_id): Path<String>,
    Json(req): Json<PublishWikiRequest>,
) -> Result<Json<serde_json::Value>, AppError> {
    let publish_url = req
        .publish_url
        .ok_or_else(|| AppError::BadRequest("publish_url is required".into()))?;
    let enable_server_render = req
        .enable_server_render
        .as_deref()
        .map(|v| v == "true")
        .unwrap_or(false);
    let result = state
        .wiki_service()
        .publish_wiki(
            &repo_id,
            auth.user_id,
            &auth.email,
            &publish_url,
            enable_server_render,
        )
        .await?;
    Ok(Json(result))
}

/// `DELETE /api/v2.1/wiki2/{repo_id}/publish/` — cancel publishing.
pub async fn unpublish_wiki(
    auth: AuthUser,
    State(state): State<Arc<AppState>>,
    Path(repo_id): Path<String>,
) -> Result<Json<serde_json::Value>, AppError> {
    state
        .wiki_service()
        .unpublish_wiki(&repo_id, auth.user_id)
        .await?;
    Ok(ok_json())
}

#[derive(Deserialize)]
pub struct UpdateConfigRequest {
    pub wiki_config: Option<serde_json::Value>,
}

#[derive(Deserialize)]
pub struct CreatePageRequest {
    pub page_name: Option<String>,
    pub current_id: Option<String>,
    pub insert_position: Option<String>,
}

#[derive(Deserialize)]
pub struct MovePageRequest {
    pub target_id: Option<String>,
    pub moved_id: Option<String>,
    pub move_position: Option<String>,
}

#[derive(Deserialize)]
pub struct PageLockRequest {
    pub is_lock_page: Option<bool>,
}

#[derive(Deserialize)]
pub struct PageConfigRequest {
    pub page_name: Option<String>,
    pub page_icon: Option<String>,
    pub page_cover: Option<String>,
}

/// `GET /api/v2.1/wiki2/{repo_id}/config/` — wiki config (navigation + pages).
pub async fn get_config(
    auth: AuthUser,
    State(state): State<Arc<AppState>>,
    Path(repo_id): Path<String>,
) -> Result<Json<serde_json::Value>, AppError> {
    Ok(Json(
        state
            .wiki_service()
            .get_wiki_config(&repo_id, auth.user_id)
            .await?,
    ))
}

/// `PUT /api/v2.1/wiki2/{repo_id}/config/` — replace the wiki config.
pub async fn update_config(
    auth: AuthUser,
    State(state): State<Arc<AppState>>,
    Path(repo_id): Path<String>,
    Json(req): Json<UpdateConfigRequest>,
) -> Result<Json<serde_json::Value>, AppError> {
    let config = req
        .wiki_config
        .ok_or_else(|| AppError::BadRequest("wiki_config invalid".into()))?;
    state
        .wiki_service()
        .update_wiki_config(&repo_id, auth.user_id, &auth.email, &config)
        .await?;
    Ok(ok_json())
}

/// `POST /api/v2.1/wiki2/{repo_id}/pages/` — create a page.
pub async fn create_page(
    auth: AuthUser,
    State(state): State<Arc<AppState>>,
    Path(repo_id): Path<String>,
    Json(req): Json<CreatePageRequest>,
) -> Result<Json<serde_json::Value>, AppError> {
    let page_name = req
        .page_name
        .ok_or_else(|| AppError::BadRequest("page_name invalid".into()))?;
    let result = state
        .wiki_service()
        .create_page(
            &repo_id,
            auth.user_id,
            &auth.email,
            &page_name,
            req.current_id.as_deref(),
            req.insert_position.as_deref(),
        )
        .await?;
    Ok(Json(result))
}

/// `PUT /api/v2.1/wiki2/{repo_id}/pages/` — move a page in the navigation.
pub async fn move_page(
    auth: AuthUser,
    State(state): State<Arc<AppState>>,
    Path(repo_id): Path<String>,
    Json(req): Json<MovePageRequest>,
) -> Result<Json<serde_json::Value>, AppError> {
    let target_id = req
        .target_id
        .ok_or_else(|| AppError::BadRequest("target_id invalid".into()))?;
    let moved_id = req
        .moved_id
        .ok_or_else(|| AppError::BadRequest("moved_id invalid".into()))?;
    let move_position = req
        .move_position
        .ok_or_else(|| AppError::BadRequest("move_position invalid".into()))?;
    state
        .wiki_service()
        .move_page(
            &repo_id,
            auth.user_id,
            &auth.email,
            &target_id,
            &moved_id,
            &move_position,
        )
        .await?;
    Ok(ok_json())
}

/// `GET /api/v2.1/wiki2/{repo_id}/page/{page_id}/` — page metadata.
pub async fn get_page(
    auth: AuthUser,
    State(state): State<Arc<AppState>>,
    Path((repo_id, page_id)): Path<(String, String)>,
) -> Result<Json<serde_json::Value>, AppError> {
    Ok(Json(
        state
            .wiki_service()
            .get_page(&repo_id, &page_id, auth.user_id)
            .await?,
    ))
}

/// `DELETE /api/v2.1/wiki2/{repo_id}/page/{page_id}/` — delete a page.
pub async fn delete_page(
    auth: AuthUser,
    State(state): State<Arc<AppState>>,
    Path((repo_id, page_id)): Path<(String, String)>,
) -> Result<Json<serde_json::Value>, AppError> {
    state
        .wiki_service()
        .delete_page(&repo_id, auth.user_id, &auth.email, &page_id)
        .await?;
    Ok(ok_json())
}

/// `PUT /api/v2.1/wiki2/{repo_id}/page/{page_id}/` — lock / unlock a page.
pub async fn set_page_locked(
    auth: AuthUser,
    State(state): State<Arc<AppState>>,
    Path((repo_id, page_id)): Path<(String, String)>,
    Json(req): Json<PageLockRequest>,
) -> Result<Json<serde_json::Value>, AppError> {
    let locked = req
        .is_lock_page
        .ok_or_else(|| AppError::BadRequest("is_lock_page is required".into()))?;
    state
        .wiki_service()
        .set_page_locked(&repo_id, auth.user_id, &auth.email, &page_id, locked)
        .await?;
    Ok(Json(serde_json::json!({ "is_locked": locked })))
}

/// `PUT /api/v2.1/wiki2/{repo_id}/page/{page_id}/config/` — page name/icon/cover.
pub async fn update_page_config(
    auth: AuthUser,
    State(state): State<Arc<AppState>>,
    Path((repo_id, page_id)): Path<(String, String)>,
    Json(req): Json<PageConfigRequest>,
) -> Result<Json<serde_json::Value>, AppError> {
    state
        .wiki_service()
        .update_page_config(
            &repo_id,
            auth.user_id,
            &auth.email,
            &page_id,
            req.page_name.as_deref(),
            req.page_icon.as_deref(),
            req.page_cover.as_deref(),
        )
        .await?;
    Ok(ok_json())
}
