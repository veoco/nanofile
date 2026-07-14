use axum::{
    Json,
    extract::{Path, Query, State},
};
use serde::Deserialize;
use serde::de::DeserializeOwned;
use std::sync::Arc;

use crate::AppState;
use crate::middleware::auth::AuthUser;
use crate::service::sharing::{link, share};
use base::error::AppError;

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
pub struct ListUploadLinksQuery {
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

    Ok(Json(serde_json::Value::Array(items)))
}

/// POST /api/v2.1/share-links/
pub async fn create_share_link_v21(
    auth: AuthUser,
    State(state): State<Arc<AppState>>,
    Json(req): Json<CreateLinkRequest>,
) -> Result<Json<serde_json::Value>, AppError> {
    let info = share::create_share_link_v21(
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

    let link_url = if info.s_type == "d" {
        format!("/d/{}/", info.token)
    } else {
        format!("/f/{}/", info.token)
    };

    Ok(Json(serde_json::json!({
        "token": info.token,
        "link": link_url,
        "repo_id": info.repo_id,
        "repo_name": null,
        "path": info.path,
        "obj_name": info.path.trim_end_matches('/').rsplit_once('/')
            .map(|(_, n)| n)
            .unwrap_or(&info.path),
        "is_dir": info.s_type == "d",
        "username": null,
        "view_cnt": 0,
        "ctime": info.created_at,
        "expire_date": info.expire_at,
        "is_expired": info.expire_at.is_some_and(|exp| chrono::Utc::now().timestamp() > exp),
        "has_password": info.has_password,
        "permissions": serde_json::json!({
            "can_edit": false,
            "can_download": true,
            "can_upload": false,
        }),
        "password": null,
        "description": info.description,
    })))
}

/// GET /api/v2.1/share-links/{token}/
pub async fn get_share_link_v21(
    auth: AuthUser,
    State(state): State<Arc<AppState>>,
    Path(token): Path<String>,
) -> Result<Json<serde_json::Value>, AppError> {
    let info = share::get_share_link_v21(&state.repos, &token, auth.user_id).await?;

    let link_url = if info.s_type == "d" {
        format!("/d/{}/", info.token)
    } else {
        format!("/f/{}/", info.token)
    };

    Ok(Json(serde_json::json!({
        "token": info.token,
        "link": link_url,
        "repo_id": info.repo_id,
        "repo_name": null,
        "path": info.path,
        "obj_name": info.path.trim_end_matches('/').rsplit_once('/')
            .map(|(_, n)| n)
            .unwrap_or(&info.path),
        "is_dir": info.s_type == "d",
        "username": null,
        "view_cnt": info.view_cnt,
        "ctime": info.created_at,
        "expire_date": info.expire_at,
        "is_expired": info.expire_at.is_some_and(|exp| chrono::Utc::now().timestamp() > exp),
        "has_password": info.has_password,
        "permissions": serde_json::json!({
            "can_edit": false,
            "can_download": true,
            "can_upload": false,
        }),
        "password": null,
        "description": info.description,
    })))
}

/// POST /api/v2.1/multi-share-links/
///
/// Creates a share link (behaves identically to POST /api/v2.1/share-links/).
/// Android client uses this endpoint.
pub async fn create_multi_share_link_v21(
    auth: AuthUser,
    State(state): State<Arc<AppState>>,
    Json(req): Json<CreateLinkRequest>,
) -> Result<Json<serde_json::Value>, AppError> {
    let info = share::create_share_link_v21(
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

    let link_url = if info.s_type == "d" {
        format!("/d/{}/", info.token)
    } else {
        format!("/f/{}/", info.token)
    };

    Ok(Json(serde_json::json!({
        "token": info.token,
        "link": link_url,
        "repo_id": info.repo_id,
        "repo_name": null,
        "path": info.path,
        "obj_name": info.path.trim_end_matches('/').rsplit_once('/')
            .map(|(_, n)| n)
            .unwrap_or(&info.path),
        "is_dir": info.s_type == "d",
        "username": null,
        "view_cnt": 0,
        "ctime": info.created_at,
        "expire_date": info.expire_at,
        "is_expired": info.expire_at.is_some_and(|exp| chrono::Utc::now().timestamp() > exp),
        "has_password": info.has_password,
        "permissions": serde_json::json!({
            "can_edit": false,
            "can_download": true,
            "can_upload": false,
        }),
        "password": null,
        "description": info.description,
    })))
}

/// DELETE /api/v2.1/share-links/{token}/
pub async fn delete_share_link_v21(
    auth: AuthUser,
    State(state): State<Arc<AppState>>,
    Path(token): Path<String>,
) -> Result<Json<serde_json::Value>, AppError> {
    let found = share::delete_share_link_v21(&state.repos, &token, auth.user_id).await?;
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
///
/// Optional query params (matching seafile API contract):
/// - `repo_id` — filter by repo
/// - `path` — filter by exact path (used with `repo_id`)
pub async fn list_upload_links_v21(
    auth: AuthUser,
    State(state): State<Arc<AppState>>,
    Query(query): Query<ListUploadLinksQuery>,
) -> Result<Json<serde_json::Value>, AppError> {
    let items = if let (Some(repo_id), Some(path)) = (&query.repo_id, &query.path) {
        link::list_upload_links_for_path(&state.repos, repo_id, path).await?
    } else {
        link::list_upload_links_v21(&state.repos, auth.user_id).await?
    };
    Ok(Json(serde_json::Value::Array(items)))
}

/// POST /api/v2.1/upload-links/
pub async fn create_upload_link_v21(
    auth: AuthUser,
    State(state): State<Arc<AppState>>,
    Json(req): Json<CreateLinkRequest>,
) -> Result<Json<serde_json::Value>, AppError> {
    let has_password = req.password.is_some();
    let path = req.path.clone();

    let info = link::create_upload_link_v21(
        &state.repos,
        &state.config,
        &req.repo_id,
        &req.path,
        req.password,
        req.expire_days,
        req.description,
        auth.user_id,
    )
    .await?;

    let obj_name = path
        .trim_end_matches('/')
        .rsplit_once('/')
        .map(|(_, n)| n.to_string())
        .unwrap_or_else(|| path.clone());

    Ok(Json(serde_json::json!({
        "token": info.token,
        "link": info.link,
        "repo_id": info.repo_id,
        "path": info.path,
        "ctime": info.created_at,
        "username": null,
        "expire_date": null,
        "is_expired": false,
        "has_password": has_password,
        "password": null,
        "description": null,
        "view_cnt": 0,
        "obj_name": obj_name,
    })))
}

/// DELETE /api/v2.1/upload-links/clean-invalid/
pub async fn clean_invalid_upload_links_v21(
    auth: AuthUser,
    State(state): State<Arc<AppState>>,
) -> Result<Json<serde_json::Value>, AppError> {
    let count = link::clean_invalid_upload_links_v21(&state.repos, auth.user_id).await?;
    Ok(Json(serde_json::json!({"success": true, "deleted": count})))
}

/// GET /api/v2.1/upload-links/{token}/
pub async fn get_upload_link_v21(
    _auth: AuthUser,
    State(state): State<Arc<AppState>>,
    Path(token): Path<String>,
) -> Result<Json<serde_json::Value>, AppError> {
    let info = link::get_upload_link_v21(&state.repos, &token).await?;
    Ok(Json(info))
}

/// PUT /api/v2.1/upload-links/{token}/
pub async fn update_upload_link_v21(
    auth: AuthUser,
    State(state): State<Arc<AppState>>,
    Path(token): Path<String>,
    Json(req): Json<UpdateLinkRequest>,
) -> Result<Json<serde_json::Value>, AppError> {
    let updated = link::update_upload_link_v21(
        &state.repos,
        &state.config,
        &token,
        auth.user_id,
        req.expire_days,
        req.password,
        req.description,
    )
    .await?;

    if !updated {
        return Err(AppError::NotFound("upload link not found".into()));
    }
    Ok(Json(serde_json::json!({"success": true})))
}

/// DELETE /api/v2.1/upload-links/{token}/
///
/// Returns bare `true` (not a JSON object) because the Android client's
/// DialogService.deleteUploadLink() uses `Single<Boolean>` and the
/// SupportResponseConverter's TypeAdapter<Boolean> cannot parse an object.
pub async fn delete_upload_link_v21(
    auth: AuthUser,
    State(state): State<Arc<AppState>>,
    Path(token): Path<String>,
) -> Result<Json<serde_json::Value>, AppError> {
    let found = link::delete_upload_link_v21_by_token(&state.repos, &token, auth.user_id).await?;
    if !found {
        return Err(AppError::NotFound("upload link not found".into()));
    }
    Ok(Json(serde_json::Value::Bool(true)))
}

/// GET /api/v2.1/upload-links/{token}/upload/
///
/// Validates the upload link and returns a short-lived upload URL.
/// Anyone with the token can call this (password checked via query param).
pub async fn get_upload_link_upload_url_v21(
    State(state): State<Arc<AppState>>,
    Path(token): Path<String>,
) -> Result<Json<serde_json::Value>, AppError> {
    // Look up the upload link
    let link = state
        .repos
        .upload_link
        .find_by_token(&token)
        .await?
        .ok_or_else(|| AppError::NotFound("Upload link not found".into()))?;

    // Check expiry
    if let Some(exp) = link.expires_at
        && chrono::Utc::now().timestamp() > exp
    {
        return Err(AppError::NotFound("Upload link has expired".into()));
    }

    // Check repo still exists
    let _repo = state
        .repos
        .repo
        .find_by_id(&link.repo_id)
        .await?
        .ok_or_else(|| AppError::NotFound("Repository not found".into()))?;

    // Get the creator's username for the token
    let creator = state
        .repos
        .user
        .find_by_id(link.creator_id)
        .await?
        .ok_or_else(|| AppError::Internal("Creator not found".into()))?;

    // Generate a short-lived upload access token
    let upload_token = state.token_manager.generate(
        &link.repo_id,
        link.creator_id,
        &creator.email,
        "upload",
        &link.path,
    );

    // Link the upload token back to the upload link so we can count uploads
    state
        .token_manager
        .set_upload_link_id(&upload_token, link.id);

    let upload_url = format!("/upload-aj/{}", upload_token);

    Ok(Json(serde_json::json!({"upload_link": upload_url})))
}

/// GET /api/v2.1/repos/{repo_id}/upload-links/
pub async fn list_repo_upload_links_v21(
    _auth: AuthUser,
    State(state): State<Arc<AppState>>,
    Path(repo_id): Path<String>,
) -> Result<Json<serde_json::Value>, AppError> {
    let items = link::list_upload_links_for_repo_v21(&state.repos, &repo_id).await?;
    Ok(Json(serde_json::Value::Array(items)))
}
