use axum::{
    Json,
    extract::{Path, State},
};
use std::sync::Arc;

use crate::AppState;
use crate::middleware::auth::AuthUser;
use base::error::AppError;

pub async fn get_metadata_config(
    _auth: AuthUser,
    State(state): State<Arc<AppState>>,
    Path(repo_id): Path<String>,
) -> Result<Json<serde_json::Value>, AppError> {
    let svc = state.metadata_service();
    let result = svc.get_metadata_config(&repo_id).await?;
    Ok(Json(result))
}

pub async fn update_metadata_config(
    _auth: AuthUser,
    State(state): State<Arc<AppState>>,
    Path(repo_id): Path<String>,
    Json(req): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, AppError> {
    let enabled = req.get("enabled").and_then(|v| v.as_bool()).unwrap_or(true);
    let svc = state.metadata_service();
    svc.update_metadata_config(&repo_id, enabled).await?;
    Ok(Json(serde_json::json!({"success": true})))
}

pub async fn get_file_tags(
    _auth: AuthUser,
    State(state): State<Arc<AppState>>,
    Path(repo_id): Path<String>,
) -> Result<Json<serde_json::Value>, AppError> {
    let svc = state.metadata_service();
    let tags = svc.get_file_tags(&repo_id).await?;
    Ok(Json(serde_json::json!({"tags": tags})))
}

pub async fn update_file_tags(
    _auth: AuthUser,
    State(state): State<Arc<AppState>>,
    Path(repo_id): Path<String>,
    Json(req): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, AppError> {
    let file_path = req.get("file_path").and_then(|v| v.as_str()).unwrap_or("");
    let tags = req.get("tags").and_then(|v| v.as_array());

    let svc = state.metadata_service();
    svc.update_file_tags(&repo_id, file_path, tags.map(|v| &**v))
        .await?;

    Ok(Json(serde_json::json!({"success": true})))
}

pub async fn related_users(
    _auth: AuthUser,
    State(state): State<Arc<AppState>>,
    Path(repo_id): Path<String>,
) -> Result<Json<serde_json::Value>, AppError> {
    let svc = state.metadata_service();
    let users = svc.related_users(&repo_id).await?;
    Ok(Json(serde_json::json!({"users": users})))
}

pub async fn custom_share_permissions(
    _auth: AuthUser,
    _state: axum::extract::State<std::sync::Arc<AppState>>,
    _repo_id: axum::extract::Path<String>,
) -> Result<Json<serde_json::Value>, AppError> {
    Ok(Json(serde_json::json!({"permissions": []})))
}

pub async fn seadoc_upload_image(
    _auth: AuthUser,
    _state: axum::extract::State<std::sync::Arc<AppState>>,
    _path: axum::extract::Path<String>,
) -> Result<Json<serde_json::Value>, AppError> {
    Ok(Json(serde_json::json!({"url": ""})))
}

pub async fn get_metadata_record(
    _auth: AuthUser,
    State(state): State<Arc<AppState>>,
    Path(repo_id): Path<String>,
) -> Result<Json<serde_json::Value>, AppError> {
    let svc = state.metadata_service();
    let records = svc.get_metadata_records(&repo_id).await?;
    Ok(Json(serde_json::json!({"records": records})))
}

pub async fn update_metadata_record(
    _auth: AuthUser,
    State(state): State<Arc<AppState>>,
    Path(repo_id): Path<String>,
    Json(req): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, AppError> {
    let file_path = req.get("file_path").and_then(|v| v.as_str()).unwrap_or("");
    let key = req.get("key").and_then(|v| v.as_str()).unwrap_or("");
    let value = req.get("value").and_then(|v| v.as_str());

    let svc = state.metadata_service();
    svc.update_metadata_record(&repo_id, file_path, key, value)
        .await?;

    Ok(Json(serde_json::json!({"success": true})))
}
