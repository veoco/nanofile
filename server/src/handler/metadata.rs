use axum::{
    Json,
    extract::{Path, Query, State},
};
use serde::Deserialize;
use std::sync::Arc;

use crate::AppState;
use crate::handler::ok_json;
use crate::middleware::auth::AuthUser;
use crate::middleware::repo_extractor::RepoPathRead;
use crate::middleware::repo_extractor::RepoPathWrite;
use base::error::AppError;

// ── metadata config ────────────────────────────────────────────────────────

pub async fn get_metadata_config(
    path: RepoPathRead,
    State(state): State<Arc<AppState>>,
) -> Result<Json<serde_json::Value>, AppError> {
    let repo_id = path.repo_id;
    let svc = state.metadata_service();
    let result = svc.get_metadata_config(&repo_id).await?;
    Ok(Json(result))
}

pub async fn update_metadata_config(
    path: RepoPathWrite,
    State(state): State<Arc<AppState>>,
    Json(req): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, AppError> {
    let repo_id = path.repo_id;
    let enabled = req.get("enabled").and_then(|v| v.as_bool()).unwrap_or(true);
    let svc = state.metadata_service();
    svc.update_metadata_config(&repo_id, enabled).await?;
    Ok(ok_json())
}

// ── file records ───────────────────────────────────────────────────────────

#[derive(Deserialize)]
pub struct RecordQuery {
    pub parent_dir: Option<String>,
    pub name: Option<String>,
    pub file_name: Option<String>,
}

/// GET /metadata/record/ — seafile mobile clients resolve a file's record_id
/// here using `parent_dir` + `file_name`.
pub async fn get_metadata_record(
    path: RepoPathRead,
    State(state): State<Arc<AppState>>,
    Query(query): Query<RecordQuery>,
) -> Result<Json<serde_json::Value>, AppError> {
    let repo_id = path.repo_id;
    let file_name = query
        .file_name
        .as_deref()
        .or(query.name.as_deref())
        .ok_or_else(|| AppError::BadRequest("file_name required".into()))?;
    let parent_dir = query.parent_dir.as_deref().unwrap_or("/");
    let svc = state.metadata_service();
    let result = svc.get_file_record(&repo_id, parent_dir, file_name).await?;
    Ok(Json(result))
}

#[derive(Deserialize)]
pub struct UpdateRecordRequest {
    pub record_id: String,
    pub data: serde_json::Value,
}

/// PUT /metadata/record/ — store non-tag metadata fields for a record.
pub async fn update_metadata_record(
    path: RepoPathWrite,
    State(state): State<Arc<AppState>>,
    Json(req): Json<UpdateRecordRequest>,
) -> Result<Json<serde_json::Value>, AppError> {
    let repo_id = path.repo_id;
    let svc = state.metadata_service();
    svc.update_file_record(&repo_id, &req.record_id, &req.data)
        .await?;
    Ok(ok_json())
}

// ── tags ───────────────────────────────────────────────────────────────────

#[derive(Deserialize)]
pub struct TagsQuery {
    pub start: Option<u32>,
    pub limit: Option<u32>,
}

/// GET /metadata/tags/ — list all repo tags (mobile clients pass
/// `start=0&limit=1000`).
pub async fn get_repo_tags(
    path: RepoPathRead,
    State(state): State<Arc<AppState>>,
    Query(query): Query<TagsQuery>,
) -> Result<Json<serde_json::Value>, AppError> {
    let repo_id = path.repo_id;
    let start = query.start.unwrap_or(0) as usize;
    let limit = query.limit.unwrap_or(1000) as usize;
    let svc = state.metadata_service();
    let result = svc.list_repo_tags(&repo_id, start, limit).await?;
    Ok(Json(result))
}

/// POST /metadata/tags/ — create tags from `{tags_data: [{_tag_name, _tag_color}]}`.
pub async fn create_repo_tags(
    path: RepoPathWrite,
    State(state): State<Arc<AppState>>,
    Json(req): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, AppError> {
    let repo_id = path.repo_id;
    let tags_data = req
        .get("tags_data")
        .cloned()
        .unwrap_or(serde_json::Value::Array(vec![]));
    let svc = state.metadata_service();
    let result = svc.create_repo_tags(&repo_id, &tags_data).await?;
    Ok(Json(result))
}

/// PUT /metadata/tags/ — rename/recolor tags from
/// `{tags_data: [{tag_id, tag: {_tag_name, _tag_color}}]}`.
pub async fn update_repo_tags(
    path: RepoPathWrite,
    State(state): State<Arc<AppState>>,
    Json(req): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, AppError> {
    let repo_id = path.repo_id;
    let tags_data = req
        .get("tags_data")
        .cloned()
        .unwrap_or(serde_json::Value::Array(vec![]));
    let svc = state.metadata_service();
    svc.update_repo_tags(&repo_id, &tags_data).await?;
    Ok(ok_json())
}

/// DELETE /metadata/tags/ — delete tags from `{tag_ids: [..]}`.
pub async fn delete_repo_tags(
    path: RepoPathWrite,
    State(state): State<Arc<AppState>>,
    Json(req): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, AppError> {
    let repo_id = path.repo_id;
    let tag_ids = req
        .get("tag_ids")
        .cloned()
        .unwrap_or(serde_json::Value::Array(vec![]));
    let svc = state.metadata_service();
    svc.delete_repo_tags(&repo_id, &tag_ids).await?;
    Ok(ok_json())
}

/// PUT /metadata/file-tags/ — set a file's tags from
/// `{file_tags_data: [{record_id, tags: [tag_id, ...]}]}` (empty array clears).
pub async fn set_file_tags(
    path: RepoPathWrite,
    State(state): State<Arc<AppState>>,
    Json(req): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, AppError> {
    let repo_id = path.repo_id;
    let data = req
        .get("file_tags_data")
        .cloned()
        .unwrap_or(serde_json::Value::Array(vec![]));
    let svc = state.metadata_service();
    svc.set_file_tags(&repo_id, &data).await?;
    Ok(ok_json())
}

/// GET /metadata/tag-files/{tag_id}/ — files carrying a tag.
///
/// Uses a manual path tuple because the route has two path parameters, which
/// `RepoPathRead` cannot extract.
pub async fn get_tag_files(
    auth: AuthUser,
    State(state): State<Arc<AppState>>,
    Path((repo_id, tag_id)): Path<(String, String)>,
) -> Result<Json<serde_json::Value>, AppError> {
    crate::domain::permission::check_repo_read_permission(
        state.repos.member.as_ref(),
        &repo_id,
        auth.user_id,
    )
    .await?;
    let tag_id = tag_id
        .parse::<i32>()
        .map_err(|_| AppError::BadRequest("invalid tag id".into()))?;
    let svc = state.metadata_service();
    let result = svc.get_tag_files(&repo_id, tag_id).await?;
    Ok(Json(result))
}

// ── tags-status (enable/disable the tag feature) ───────────────────────────

pub async fn get_tags_status(
    path: RepoPathRead,
    State(state): State<Arc<AppState>>,
) -> Result<Json<serde_json::Value>, AppError> {
    let repo_id = path.repo_id;
    let config = state
        .metadata_service()
        .get_metadata_config(&repo_id)
        .await?;
    let enabled = config
        .get("tags_enabled")
        .and_then(|v| v.as_bool())
        .unwrap_or(true);
    Ok(Json(
        serde_json::json!({ "enabled": enabled, "lang": "en" }),
    ))
}

pub async fn update_tags_status(
    path: RepoPathWrite,
    State(state): State<Arc<AppState>>,
) -> Result<Json<serde_json::Value>, AppError> {
    let repo_id = path.repo_id;
    state
        .metadata_service()
        .update_tags_enabled(&repo_id, true)
        .await?;
    Ok(ok_json())
}

pub async fn delete_tags_status(
    path: RepoPathWrite,
    State(state): State<Arc<AppState>>,
) -> Result<Json<serde_json::Value>, AppError> {
    let repo_id = path.repo_id;
    state
        .metadata_service()
        .update_tags_enabled(&repo_id, false)
        .await?;
    Ok(ok_json())
}

// ── related users / misc ───────────────────────────────────────────────────

pub async fn related_users(
    path: RepoPathRead,
    State(state): State<Arc<AppState>>,
) -> Result<Json<serde_json::Value>, AppError> {
    let repo_id = path.repo_id;
    let svc = state.metadata_service();
    let users = svc.related_users(&repo_id).await?;
    Ok(Json(serde_json::json!({"users": users})))
}

pub async fn custom_share_permissions(
    _path: RepoPathRead,
    _state: axum::extract::State<std::sync::Arc<AppState>>,
) -> Result<Json<serde_json::Value>, AppError> {
    Ok(Json(serde_json::json!({"permissions": []})))
}

pub async fn seadoc_upload_image(
    _path: RepoPathRead,
    _state: axum::extract::State<std::sync::Arc<AppState>>,
) -> Result<Json<serde_json::Value>, AppError> {
    Ok(Json(serde_json::json!({"url": ""})))
}
