use axum::{Json, extract::State};
use std::sync::Arc;

use crate::AppState;
use crate::handler::ok_json;
use crate::middleware::repo_extractor::RepoPathRead;
use crate::middleware::repo_extractor::RepoPathWrite;
use base::error::AppError;

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

pub async fn get_file_tags(
    path: RepoPathRead,
    State(state): State<Arc<AppState>>,
) -> Result<Json<serde_json::Value>, AppError> {
    let repo_id = path.repo_id;
    let svc = state.metadata_service();
    let tags = svc.get_file_tags(&repo_id).await?;
    Ok(Json(serde_json::json!({"tags": tags})))
}

pub async fn update_file_tags(
    path: RepoPathWrite,
    State(state): State<Arc<AppState>>,
    Json(req): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, AppError> {
    let repo_id = path.repo_id;
    let file_path = req.get("file_path").and_then(|v| v.as_str()).unwrap_or("");
    let tags = req.get("tags").and_then(|v| v.as_array());

    let svc = state.metadata_service();
    svc.update_file_tags(&repo_id, file_path, tags.map(|v| &**v))
        .await?;

    Ok(ok_json())
}

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

pub async fn get_metadata_record(
    path: RepoPathRead,
    State(state): State<Arc<AppState>>,
) -> Result<Json<serde_json::Value>, AppError> {
    let repo_id = path.repo_id;
    let svc = state.metadata_service();
    let records = svc.get_metadata_records(&repo_id).await?;
    Ok(Json(serde_json::json!({"records": records})))
}

pub async fn update_metadata_record(
    path: RepoPathWrite,
    State(state): State<Arc<AppState>>,
    Json(req): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, AppError> {
    let repo_id = path.repo_id;
    let file_path = req.get("file_path").and_then(|v| v.as_str()).unwrap_or("");
    let key = req.get("key").and_then(|v| v.as_str()).unwrap_or("");
    let value = req.get("value").and_then(|v| v.as_str());

    let svc = state.metadata_service();
    svc.update_metadata_record(&repo_id, file_path, key, value)
        .await?;

    Ok(ok_json())
}
