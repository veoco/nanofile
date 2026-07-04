use axum::{
    Json,
    extract::{Path, Query, State},
};
use serde::Deserialize;
use serde::de::DeserializeOwned;
use std::sync::Arc;

use crate::AppState;
use crate::auth::middleware::AuthUser;
use crate::error::AppError;
use crate::sharing::service::{link, share};

/// Custom deserializer that maps JSON `null` to `Some(None)` and a present
/// value to `Some(Some(v))`, while a missing field remains `None` (via
/// `#[serde(default)]`). This distinguishes "don't update" from "set to null".
fn deserialize_nullable<'de, D, T>(d: D) -> Result<Option<Option<T>>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: DeserializeOwned,
{
    Ok(Some(Option::<T>::deserialize(d)?))
}

#[derive(Deserialize)]
pub struct CreateLinkRequest {
    pub repo_id: String,
    pub path: String,
    pub password: Option<String>,
    pub expire_days: Option<i64>,
    pub description: Option<String>,
}

#[derive(Deserialize)]
pub struct ListShareLinksQuery {
    pub repo_id: Option<String>,
    pub path: Option<String>,
}

#[derive(Deserialize)]
pub struct UpdateLinkRequest {
    #[serde(default, deserialize_with = "deserialize_nullable")]
    pub password: Option<Option<String>>,
    #[serde(default, deserialize_with = "deserialize_nullable")]
    pub expire_days: Option<Option<i64>>,
    #[serde(default, deserialize_with = "deserialize_nullable")]
    pub description: Option<Option<String>>,
}

/// GET /api/v2.1/share-links/
///
/// Optional query params (matching seafile API contract):
/// - `repo_id` — filter by repo
/// - `path` — filter by exact path (used with `repo_id`)
pub async fn list_share_links_v21(
    auth: AuthUser,
    State(state): State<Arc<AppState>>,
    Query(query): Query<ListShareLinksQuery>,
) -> Result<Json<serde_json::Value>, AppError> {
    let infos = if let (Some(repo_id), Some(path)) = (&query.repo_id, &query.path) {
        share::list_share_links_for_path(&state.repos, repo_id, path).await?
    } else {
        share::list_share_links(&state.repos, auth.user_id).await?
    };
    let items: Vec<serde_json::Value> = infos
        .into_iter()
        .map(|l| {
            serde_json::json!({
                "token": l.token,
                "link": l.link,
                "repo_id": l.repo_id,
                "path": l.path,
                "created_at": l.created_at,
                "has_password": l.has_password,
                "expire_at": l.expire_at,
                "s_type": l.s_type,
                "view_cnt": l.view_cnt,
                "description": l.description,
            })
        })
        .collect();

    Ok(Json(serde_json::json!({"share_link_list": items})))
}

/// POST /api/v2.1/share-links/
pub async fn create_share_link_v21(
    auth: AuthUser,
    State(state): State<Arc<AppState>>,
    Json(req): Json<CreateLinkRequest>,
) -> Result<Json<serde_json::Value>, AppError> {
    let result = share::create_share_link_v21(
        state.db.as_ref(),
        &state.repos,
        &state.config,
        &req.repo_id,
        &req.path,
        req.password.as_deref(),
        req.expire_days,
        req.description.as_deref(),
        auth.user_id,
    )
    .await?;

    Ok(Json(
        serde_json::json!({"token": result.token, "s_type": result.s_type}),
    ))
}

/// DELETE /api/v2.1/share-links/{token}/
pub async fn delete_share_link_v21(
    auth: AuthUser,
    State(state): State<Arc<AppState>>,
    Path(token): Path<String>,
) -> Result<Json<serde_json::Value>, AppError> {
    let found = share::delete_share_link_v21(state.db.as_ref(), &token, auth.user_id).await?;
    if !found {
        return Err(AppError::NotFound("share link not found".into()));
    }
    Ok(Json(serde_json::json!({"success": true})))
}

/// PUT /api/v2.1/share-links/{token}/
pub async fn update_share_link_v21(
    auth: AuthUser,
    State(state): State<Arc<AppState>>,
    Path(token): Path<String>,
    Json(req): Json<UpdateLinkRequest>,
) -> Result<Json<serde_json::Value>, AppError> {
    let info = share::update_share_link_v21(
        state.db.as_ref(),
        &state.config,
        &state.repos,
        &token,
        auth.user_id,
        req.password,
        req.expire_days,
        req.description,
    )
    .await?;

    Ok(Json(serde_json::json!({
        "token": info.token,
        "link": info.link,
        "repo_id": info.repo_id,
        "path": info.path,
        "created_at": info.created_at,
        "has_password": info.has_password,
        "expire_at": info.expire_at,
        "s_type": info.s_type,
        "view_cnt": info.view_cnt,
        "description": info.description,
    })))
}

/// GET /api/v2.1/upload-links/
pub async fn list_upload_links_v21(
    auth: AuthUser,
    State(state): State<Arc<AppState>>,
) -> Result<Json<serde_json::Value>, AppError> {
    let items = link::list_upload_links_v21(&state.repos, auth.user_id).await?;
    Ok(Json(serde_json::json!({"upload_link_list": items})))
}

/// POST /api/v2.1/upload-links/
pub async fn create_upload_link_v21(
    auth: AuthUser,
    State(state): State<Arc<AppState>>,
    Json(req): Json<CreateLinkRequest>,
) -> Result<Json<serde_json::Value>, AppError> {
    let token = link::create_upload_link_v21(
        state.db.as_ref(),
        &req.repo_id,
        &req.path,
        req.password,
        req.expire_days,
        auth.user_id,
    )
    .await?;

    Ok(Json(serde_json::json!({"token": token})))
}

/// DELETE /api/v2.1/upload-links/{id}/
///
/// Returns bare `true` (not a JSON object) because the Android client's
/// DialogService.deleteUploadLink() uses `Single<Boolean>` and the
/// SupportResponseConverter's TypeAdapter<Boolean> cannot parse an object.
pub async fn delete_upload_link_v21(
    auth: AuthUser,
    State(state): State<Arc<AppState>>,
    Path(id): Path<i32>,
) -> Result<Json<serde_json::Value>, AppError> {
    let found = link::delete_upload_link_v21(&state.repos, id, auth.user_id).await?;
    if !found {
        return Err(AppError::NotFound("upload link not found".into()));
    }
    Ok(Json(serde_json::Value::Bool(true)))
}
