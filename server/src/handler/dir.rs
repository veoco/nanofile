use axum::http::{HeaderMap, HeaderName, HeaderValue};
use axum::{
    Json, Router,
    body::Body,
    extract::{Path, Query, State},
    http::Request,
    response::{IntoResponse, Response},
};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;

use crate::AppState;
use crate::middleware::auth::AuthUser;
use crate::middleware::repo_extractor::{RepoPathRead, RepoPathWrite};
use base::error::AppError;
use base::sanitize::safe_normalize_path;
use infra::common::DirEntry;

#[derive(Deserialize)]
pub struct DirQuery {
    pub p: Option<String>,
    pub t: Option<String>,
    pub recursive: Option<String>,
}

pub fn dir_routes() -> Router<Arc<AppState>> {
    Router::new()
        .route(
            "/{repo_id}/dir/",
            axum::routing::get(list_dir)
                .post(dir_post_handler)
                .delete(delete_dir),
        )
        .route("/{repo_id}/dir/move/", axum::routing::post(move_dir))
        .route("/{repo_id}/dir/rename/", axum::routing::post(rename_dir))
        .route(
            "/{repo_id}/dir/shared_items/",
            axum::routing::get(dir_shared_items),
        )
        .route(
            "/{repo_id}/dir/sub_repo/",
            axum::routing::get(create_sub_repo),
        )
}

pub async fn list_dir(
    access: RepoPathRead,
    State(state): State<Arc<AppState>>,
    Query(query): Query<DirQuery>,
) -> Result<impl IntoResponse, AppError> {
    let repo_id = &access.repo_id;

    let path = safe_normalize_path(&query.p.unwrap_or_else(|| "/".to_string()))
        .map_err(|e| AppError::BadRequest(format!("Invalid path: {e}")))?;
    if let Some(ref r) = query.recursive
        && r != "0"
        && r != "1"
    {
        return Err(AppError::BadRequest(
            "If you want to get recursive dir entries, you should set 'recursive' argument as '1'."
                .into(),
        ));
    }
    if let Some(ref t) = query.t
        && t != "f"
        && t != "d"
    {
        return Err(AppError::BadRequest(
            "'t'(type) should be 'f' or 'd'.".into(),
        ));
    }

    let svc = state.dir_service();

    if query.recursive.as_deref() == Some("1") {
        let (dir_id, all_entries) = svc.list_dir_recursive(repo_id, &path).await?;
        let filtered: Vec<DirEntry> = match query.t.as_deref() {
            Some("f") => all_entries
                .into_iter()
                .filter(|e| e.entry_type == "file")
                .collect(),
            Some("d") => all_entries
                .into_iter()
                .filter(|e| e.entry_type == "dir")
                .collect(),
            _ => all_entries,
        };
        let mut headers = HeaderMap::new();
        if !dir_id.is_empty() {
            headers.insert("oid", dir_id.parse().unwrap());
        }
        headers.insert("dir_perm", "rw".parse().unwrap());
        return Ok((headers, Json(filtered)));
    }

    let (dir_id, entries) = svc.list_dir(repo_id, &path).await?;
    let mut headers = HeaderMap::new();
    if !dir_id.is_empty() {
        headers.insert("oid", dir_id.parse().unwrap());
    }
    headers.insert("dir_perm", "rw".parse().unwrap());
    Ok((headers, Json(entries)))
}

pub async fn dir_post_handler(
    access: RepoPathWrite,
    State(state): State<Arc<AppState>>,
    Query(query): Query<DirQuery>,
    req: Request<Body>,
) -> Result<Json<serde_json::Value>, AppError> {
    let repo_id = &access.repo_id;

    let (_parts, body) = req.into_parts();
    let bytes = axum::body::to_bytes(body, usize::MAX)
        .await
        .map_err(|e| AppError::Internal(e.to_string()))?;

    let (op, p, newname): (Option<String>, Option<String>, Option<String>) =
        if let Ok(json_val) = serde_json::from_slice::<serde_json::Value>(&bytes) {
            let op = json_val
                .get("operation")
                .and_then(|v| v.as_str())
                .map(String::from);
            let p = json_val.get("p").and_then(|v| v.as_str()).map(String::from);
            let newname = json_val
                .get("newname")
                .and_then(|v| v.as_str())
                .map(String::from);
            (op, p, newname)
        } else if let Ok(form) = serde_urlencoded::from_bytes::<HashMap<String, String>>(&bytes) {
            (
                form.get("operation").cloned(),
                form.get("p").cloned(),
                form.get("newname").cloned(),
            )
        } else {
            let op = infra::common::util::extract_multipart_field(&bytes, "operation");
            let p = infra::common::util::extract_multipart_field(&bytes, "p");
            let newname = infra::common::util::extract_multipart_field(&bytes, "newname");
            (op, p, newname)
        };

    let path = p
        .or_else(|| query.p.clone())
        .ok_or_else(|| AppError::BadRequest("path required".into()))?;
    let path = safe_normalize_path(&path)
        .map_err(|e| AppError::BadRequest(format!("Invalid path: {e}")))?;

    let svc = state.dir_service();

    match op.as_deref() {
        Some("rename") => {
            let newname = newname.ok_or_else(|| AppError::BadRequest("newname required".into()))?;
            svc.rename_dir_entry(
                repo_id,
                &path,
                &newname,
                &access.user.email,
                access.user.user_id,
            )
            .await?;
            Ok(Json(serde_json::Value::String("success".to_string())))
        }
        _ => {
            svc.create_dir(repo_id, &path, &access.user.email, access.user.user_id)
                .await?;
            Ok(Json(serde_json::Value::String("success".to_string())))
        }
    }
}

pub async fn delete_dir(
    access: RepoPathWrite,
    State(state): State<Arc<AppState>>,
    Query(query): Query<DirQuery>,
) -> Result<(), AppError> {
    let repo_id = &access.repo_id;

    let path = safe_normalize_path(
        &query
            .p
            .ok_or_else(|| AppError::BadRequest("path is required".into()))?,
    )
    .map_err(|e| AppError::BadRequest(format!("Invalid path: {e}")))?;

    let svc = state.dir_service();
    svc.delete_dir(repo_id, &path, &access.user.email, access.user.user_id)
        .await
}

#[derive(Deserialize)]
pub struct MoveDirRequest {
    pub repo_id: String,
    pub p: String,
    pub new_parent_dir: String,
}

pub async fn move_dir(
    auth: AuthUser,
    State(state): State<Arc<AppState>>,
    Json(req): Json<MoveDirRequest>,
) -> Result<(), AppError> {
    crate::domain::permission::check_repo_write_permission(
        state.repos.member.as_ref(),
        &req.repo_id,
        auth.user_id,
    )
    .await?;

    let svc = state.dir_service();
    svc.move_dir(
        &req.repo_id,
        &req.p,
        &req.new_parent_dir,
        &auth.email,
        auth.user_id,
    )
    .await
}

#[derive(Deserialize)]
pub struct RenameDirRequest {
    pub repo_id: String,
    pub p: String,
    pub new_name: String,
}

pub async fn rename_dir(
    auth: AuthUser,
    State(state): State<Arc<AppState>>,
    Json(req): Json<RenameDirRequest>,
) -> Result<(), AppError> {
    crate::domain::permission::check_repo_write_permission(
        state.repos.member.as_ref(),
        &req.repo_id,
        auth.user_id,
    )
    .await?;

    let path = safe_normalize_path(&req.p)
        .map_err(|e| AppError::BadRequest(format!("Invalid path: {e}")))?;
    let svc = state.dir_service();
    svc.rename_dir_entry(
        &req.repo_id,
        &path,
        &req.new_name,
        &auth.email,
        auth.user_id,
    )
    .await
}

#[derive(Serialize)]
pub struct DirSharedItemsResponse {
    pub shared_items: Vec<serde_json::Value>,
}

pub async fn dir_shared_items(
    access: RepoPathRead,
    State(state): State<Arc<AppState>>,
    Query(query): Query<DirQuery>,
) -> Result<Json<DirSharedItemsResponse>, AppError> {
    let repo_id = &access.repo_id;

    let path = safe_normalize_path(&query.p.unwrap_or_else(|| "/".to_string()))
        .map_err(|e| AppError::BadRequest(format!("Invalid path: {e}")))?;
    let svc = state.dir_service();
    let items = svc.get_dir_shared_items(repo_id, &path).await?;

    Ok(Json(DirSharedItemsResponse {
        shared_items: items,
    }))
}

pub async fn create_sub_repo(
    access: RepoPathWrite,
    State(state): State<Arc<AppState>>,
    Query(query): Query<DirQuery>,
) -> Result<Json<serde_json::Value>, AppError> {
    let repo_id = &access.repo_id;

    let path = safe_normalize_path(
        &query
            .p
            .ok_or_else(|| AppError::BadRequest("path required".into()))?,
    )
    .map_err(|e| AppError::BadRequest(format!("Invalid path: {e}")))?;

    let svc = state.dir_service();
    let result = svc
        .create_sub_repo(repo_id, &path, &access.user.email, access.user.user_id)
        .await?;

    Ok(Json(result))
}

// ── v2.1 API handlers ───────────────────────────────────────────────────

#[derive(Deserialize)]
pub struct V21DirQuery {
    pub p: Option<String>,
    pub t: Option<String>,
    pub recursive: Option<String>,
    pub with_thumbnail: Option<bool>,
}

pub async fn delete_dirent_v21(
    auth: AuthUser,
    State(state): State<Arc<AppState>>,
    Path((repo_id, obj)): Path<(String, String)>,
    Query(query): Query<V21DirQuery>,
) -> Result<Json<serde_json::value::Value>, AppError> {
    crate::domain::permission::check_repo_write_permission(
        state.repos.member.as_ref(),
        &repo_id,
        auth.user_id,
    )
    .await?;

    let path = query
        .p
        .as_deref()
        .ok_or_else(|| AppError::BadRequest("path required".into()))?;
    let normalized = safe_normalize_path(path)
        .map_err(|e| AppError::BadRequest(format!("Invalid path: {e}")))?;

    let svc = state.dir_service();
    svc.delete_dirent(&repo_id, &obj, &normalized, &auth.email, auth.user_id)
        .await?;

    // Clean up cached thumbnails for deleted files
    if obj == "file" {
        let thumb_svc = state.thumbnail_service();
        thumb_svc.cleanup(&repo_id, &normalized).await;
    }

    Ok(Json(serde_json::json!({"success": true})))
}

pub async fn list_dir_v21(
    access: RepoPathRead,
    State(state): State<Arc<AppState>>,
    Query(query): Query<V21DirQuery>,
) -> Result<Response, AppError> {
    let repo_id = &access.repo_id;

    let path = query.p.as_deref().unwrap_or("/");
    let normalized = safe_normalize_path(path)
        .map_err(|e| AppError::BadRequest(format!("Invalid path: {e}")))?;
    if let Some(ref r) = query.recursive
        && r != "0"
        && r != "1"
    {
        return Err(AppError::BadRequest(
            "If you want to get recursive dir entries, you should set 'recursive' argument as '1'."
                .into(),
        ));
    }
    if let Some(ref t) = query.t
        && t != "f"
        && t != "d"
    {
        return Err(AppError::BadRequest(
            "'t'(type) should be 'f' or 'd'.".into(),
        ));
    }

    let svc = state.dir_service();

    if query.recursive.as_deref() == Some("1") {
        let (dir_id, all_entries) = svc.list_dir_recursive(repo_id, &normalized).await?;
        let dirent_list: Vec<DirEntry> = match query.t.as_deref() {
            Some("f") => all_entries
                .into_iter()
                .filter(|e| e.entry_type == "file")
                .collect(),
            Some("d") => all_entries
                .into_iter()
                .filter(|e| e.entry_type == "dir")
                .collect(),
            _ => all_entries,
        };

        let user_perm = state
            .repos
            .member
            .find_by_repo_and_user(repo_id, access.user.user_id)
            .await?
            .map(|m| m.permission)
            .unwrap_or_else(|| "rw".to_string());

        let mut headers = HeaderMap::new();
        if !dir_id.is_empty() {
            headers.insert(
                HeaderName::from_static("oid"),
                HeaderValue::from_str(&dir_id).unwrap_or_else(|_| {
                    HeaderValue::from_static("0000000000000000000000000000000000000000")
                }),
            );
        }
        let body = serde_json::json!({
            "user_perm": user_perm,
            "dir_id": dir_id,
            "dirent_list": dirent_list,
        });
        return Ok((headers, Json(body)).into_response());
    }

    let (dir_id, entries) = svc.list_dir(repo_id, &normalized).await?;

    let json_body = svc
        .build_list_dir_v21_json(
            repo_id,
            &normalized,
            access.user.user_id,
            query.with_thumbnail.unwrap_or(false),
            entries,
            dir_id.clone(),
        )
        .await?;

    let mut headers = HeaderMap::new();
    if !dir_id.is_empty() {
        headers.insert(
            HeaderName::from_static("oid"),
            HeaderValue::from_str(&dir_id).unwrap_or_else(|_| {
                HeaderValue::from_static("0000000000000000000000000000000000000000")
            }),
        );
    }

    Ok((headers, Json(json_body)).into_response())
}

#[derive(Deserialize)]
pub struct CreateDirBody {
    p: Option<String>,
    #[serde(rename = "operation")]
    _operation: Option<String>,
}

pub async fn create_dir_v21(
    access: RepoPathWrite,
    State(state): State<Arc<AppState>>,
    Query(query): Query<V21DirQuery>,
    Json(body): Json<CreateDirBody>,
) -> Result<Json<serde_json::Value>, AppError> {
    let repo_id = &access.repo_id;

    let path = body
        .p
        .or(query.p)
        .ok_or_else(|| AppError::BadRequest("path (p) required".into()))?;
    let path = safe_normalize_path(&path)
        .map_err(|e| AppError::BadRequest(format!("Invalid path: {e}")))?;

    let svc = state.dir_service();
    svc.create_dir(repo_id, &path, &access.user.email, access.user.user_id)
        .await?;

    Ok(Json(serde_json::json!({"success": true})))
}

#[derive(Deserialize)]
pub struct DirDetailQuery {
    pub path: Option<String>,
}

pub async fn dir_detail_v21(
    access: RepoPathRead,
    State(state): State<Arc<AppState>>,
    Query(query): Query<DirDetailQuery>,
) -> Result<Json<serde_json::Value>, AppError> {
    let repo_id = &access.repo_id;

    let path = query
        .path
        .ok_or_else(|| AppError::BadRequest("path required".into()))?;
    if path == "/" || path.is_empty() {
        return Err(AppError::BadRequest("path invalid.".into()));
    }
    let normalized = safe_normalize_path(&path)
        .map_err(|e| AppError::BadRequest(format!("Invalid path: {e}")))?;

    let svc = state.dir_service();
    let result = svc
        .dir_detail(repo_id, &normalized, access.user.user_id)
        .await?;

    Ok(Json(result))
}
