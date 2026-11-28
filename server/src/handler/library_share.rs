//! Administrator-managed library sharing endpoints.
//!
//! The official desktop client has always known how to manage a library's user
//! shares through `dir/shared_items/`; nanofile only ever answered that path with
//! download links. These handlers restore the upstream meanings (a bare array of
//! user shares, `PUT` to grant, `POST` to change, `DELETE` to revoke), add the
//! recipient-side "leave share" action, the client's user completer, and the
//! nanofile-specific visibility switch that also supports publishing a library to
//! every account.
//!
//! Policy: only an administrator who owns the library may change its sharing;
//! groups and folder paths are rejected because the data model does not have
//! them.

use std::collections::HashMap;

use axum::{
    Json, Router,
    body::{Body, Bytes},
    extract::{Path, Query, State},
    http::HeaderMap,
    routing::{delete, get},
};
use serde::Deserialize;
use std::sync::Arc;

use crate::AppState;
use crate::handler::{MAX_SMALL_BODY_BYTES, ok_json, read_body_limited};
use crate::middleware::auth::AuthUser;
use crate::service::sharing::library::LibraryShareService;
use base::error::AppError;

#[derive(Deserialize, Default)]
pub struct SharedItemsQuery {
    p: Option<String>,
    share_type: Option<String>,
    username: Option<String>,
    permission: Option<String>,
}

#[derive(Deserialize, Default)]
pub struct ShareBody {
    permission: Option<String>,
    username: Option<String>,
    share_type: Option<String>,
    visibility: Option<String>,
}

fn require_library_root(p: Option<&str>) -> Result<(), AppError> {
    let path = base::sanitize::safe_normalize_path(p.unwrap_or("/"))
        .map_err(|e| AppError::BadRequest(format!("Invalid path: {e}")))?;
    if path != "/" {
        return Err(AppError::BadRequest(
            "only a whole library can be shared".into(),
        ));
    }
    Ok(())
}

fn require_user_share_type(share_type: Option<&str>) -> Result<(), AppError> {
    match share_type.map(str::trim) {
        None | Some("") | Some("user") => Ok(()),
        Some("group") => Err(AppError::BadRequest(
            "group sharing is not supported".into(),
        )),
        Some(other) => Err(AppError::BadRequest(format!(
            "unsupported share_type: {other}"
        ))),
    }
}

/// Parse the small metadata body the official client sends as a form and the web
/// UI sends as JSON (multipart for the Android client is accepted for parity).
async fn read_share_body(headers: &HeaderMap, body: Body) -> Result<ShareBody, AppError> {
    let bytes: Bytes = read_body_limited(body, MAX_SMALL_BODY_BYTES).await?;
    if bytes.is_empty() {
        return Ok(ShareBody::default());
    }
    let content_type = headers
        .get(axum::http::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default();
    if content_type.contains("multipart/form-data") {
        let field = |name: &str| infra::common::util::extract_multipart_field(&bytes, name);
        Ok(ShareBody {
            permission: field("permission"),
            username: field("username"),
            share_type: field("share_type"),
            visibility: field("visibility"),
        })
    } else if content_type.contains("json") {
        serde_json::from_slice(&bytes)
            .map_err(|e| AppError::BadRequest(format!("invalid JSON body: {e}")))
    } else {
        serde_urlencoded::from_bytes(&bytes)
            .map_err(|e| AppError::BadRequest(format!("invalid form body: {e}")))
    }
}

fn shared_item_routes() -> Router<Arc<AppState>> {
    Router::new().route(
        "/{repo_id}/dir/shared_items/",
        get(list_shared_items)
            .put(add_shared_item)
            .post(update_shared_item)
            .delete(remove_shared_item),
    )
}

fn share_routes() -> Router<Arc<AppState>> {
    Router::new().route("/{repo_id}/", delete(leave_shared_repo))
}

/// Register the `/api2/beshared-repos` and `/api2/repos/{repo_id}/dir/shared_items/`
/// surfaces plus the administrator's user search.
pub fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .nest("/beshared-repos", share_routes())
        .route("/search-user/", get(search_user))
}

pub fn repo_nested_routes() -> Router<Arc<AppState>> {
    shared_item_routes()
}

fn service(state: &AppState) -> LibraryShareService {
    LibraryShareService::new(state.repos.clone(), state.config().server.site_url_origin())
}

async fn list_shared_items(
    auth: AuthUser,
    State(state): State<Arc<AppState>>,
    Path(repo_id): Path<String>,
    Query(query): Query<SharedItemsQuery>,
) -> Result<Json<Vec<serde_json::Value>>, AppError> {
    require_library_root(query.p.as_deref())?;
    auth.ensure_repo_allowed(&repo_id, false)?;
    Ok(Json(
        service(&state).list_members(&repo_id, auth.user_id).await?,
    ))
}

async fn add_shared_item(
    auth: AuthUser,
    State(state): State<Arc<AppState>>,
    Path(repo_id): Path<String>,
    Query(query): Query<SharedItemsQuery>,
    headers: HeaderMap,
    body: Body,
) -> Result<Json<serde_json::Value>, AppError> {
    require_library_root(query.p.as_deref())?;
    auth.ensure_repo_allowed(&repo_id, true)?;
    let body = read_share_body(&headers, body).await?;
    require_user_share_type(body.share_type.as_deref().or(query.share_type.as_deref()))?;
    let username = body
        .username
        .as_deref()
        .or(query.username.as_deref())
        .ok_or_else(|| AppError::BadRequest("username is required".into()))?;
    let permission = body.permission.as_deref().or(query.permission.as_deref());
    let info = service(&state)
        .add_member(&repo_id, auth.user_id, username, permission)
        .await?;
    state.left_panel_cache.clear_all();
    Ok(Json(serde_json::json!({
        "success": [info],
        "failed": [],
        "virtual_repo_id": serde_json::Value::Null,
    })))
}

async fn update_shared_item(
    auth: AuthUser,
    State(state): State<Arc<AppState>>,
    Path(repo_id): Path<String>,
    Query(query): Query<SharedItemsQuery>,
    headers: HeaderMap,
    body: Body,
) -> Result<Json<serde_json::Value>, AppError> {
    require_library_root(query.p.as_deref())?;
    auth.ensure_repo_allowed(&repo_id, true)?;
    let body = read_share_body(&headers, body).await?;
    require_user_share_type(query.share_type.as_deref().or(body.share_type.as_deref()))?;
    let username = query
        .username
        .as_deref()
        .or(body.username.as_deref())
        .ok_or_else(|| AppError::BadRequest("username is required".into()))?;
    let permission = body.permission.as_deref().or(query.permission.as_deref());
    service(&state)
        .update_member(&repo_id, auth.user_id, username, permission)
        .await?;
    state.left_panel_cache.clear_all();
    Ok(ok_json())
}

async fn remove_shared_item(
    auth: AuthUser,
    State(state): State<Arc<AppState>>,
    Path(repo_id): Path<String>,
    Query(query): Query<SharedItemsQuery>,
) -> Result<Json<serde_json::Value>, AppError> {
    require_library_root(query.p.as_deref())?;
    auth.ensure_repo_allowed(&repo_id, true)?;
    require_user_share_type(query.share_type.as_deref())?;
    let username = query
        .username
        .as_deref()
        .ok_or_else(|| AppError::BadRequest("username is required".into()))?;
    service(&state)
        .remove_member(&repo_id, auth.user_id, username)
        .await?;
    state.left_panel_cache.clear_all();
    Ok(ok_json())
}

/// `DELETE /api2/beshared-repos/{repo_id}/?share_type=personal`
///
/// The recipient's "Leave share" action. The desktop client sends `from=<owner>`
/// as well; the server derives the owner from the row and ignores it.
async fn leave_shared_repo(
    auth: AuthUser,
    State(state): State<Arc<AppState>>,
    Path(repo_id): Path<String>,
    Query(query): Query<HashMap<String, String>>,
) -> Result<Json<serde_json::Value>, AppError> {
    auth.ensure_repo_allowed(&repo_id, true)?;
    match query.get("share_type").map(String::as_str) {
        None | Some("") | Some("personal") => {}
        Some("group") => {
            return Err(AppError::BadRequest(
                "group sharing is not supported".into(),
            ));
        }
        Some("public") => {
            return Err(AppError::BadRequest(
                "a public library has no share to leave".into(),
            ));
        }
        Some(other) => {
            return Err(AppError::BadRequest(format!(
                "unsupported share_type: {other}"
            )));
        }
    }
    service(&state).leave_share(&repo_id, auth.user_id).await?;
    state.left_panel_cache.clear_all();
    Ok(ok_json())
}

async fn search_user(
    auth: AuthUser,
    State(state): State<Arc<AppState>>,
    Query(query): Query<HashMap<String, String>>,
) -> Result<Json<serde_json::Value>, AppError> {
    auth.require_admin(&state).await?;
    let q = query.get("q").map(String::as_str).unwrap_or("");
    Ok(Json(serde_json::json!({
        "users": service(&state).search_users(q).await?,
    })))
}

/// `PUT /api/repos/{repo_id}/visibility/` — publish, restrict or unpublish.
async fn set_visibility(
    auth: AuthUser,
    State(state): State<Arc<AppState>>,
    Path(repo_id): Path<String>,
    headers: HeaderMap,
    body: Body,
) -> Result<Json<serde_json::Value>, AppError> {
    auth.ensure_repo_allowed(&repo_id, true)?;
    let body = read_share_body(&headers, body).await?;
    let visibility = body
        .visibility
        .as_deref()
        .ok_or_else(|| AppError::BadRequest("visibility is required".into()))?;
    service(&state)
        .set_visibility(&repo_id, auth.user_id, visibility)
        .await?;
    state.left_panel_cache.clear_all();
    Ok(ok_json())
}

/// The Nanofile-only library routes under `/api/repos/{repo_id}/`.
///
/// Library visibility (private / shared / public) is this server's own
/// administrator-managed sharing model; Seafile has no such endpoint.
pub fn nanofile_repo_routes() -> Router<Arc<AppState>> {
    Router::new().route("/{repo_id}/visibility/", axum::routing::put(set_visibility))
}
