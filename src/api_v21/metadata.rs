use axum::{
    Json,
    extract::{Path, State},
};
use sea_orm::{ActiveModelTrait, ColumnTrait, EntityTrait, QueryFilter, Set};
use std::sync::Arc;

use crate::AppState;
use crate::auth::middleware::AuthUser;
use crate::entity::{file_tag, metadata_config, metadata_record};
use crate::error::AppError;

/// GET /api/v2.1/repos/{repo_id}/metadata/
pub async fn get_metadata_config(
    _auth: AuthUser,
    State(state): State<Arc<AppState>>,
    Path(repo_id): Path<String>,
) -> Result<Json<serde_json::Value>, AppError> {
    let config = metadata_config::Entity::find()
        .filter(metadata_config::Column::RepoId.eq(&repo_id))
        .one(state.db.as_ref())
        .await?;

    match config {
        Some(c) => Ok(Json(serde_json::json!({"enabled": c.enabled}))),
        None => Ok(Json(serde_json::json!({"enabled": false}))),
    }
}

/// PUT /api/v2.1/repos/{repo_id}/metadata/
pub async fn update_metadata_config(
    _auth: AuthUser,
    State(state): State<Arc<AppState>>,
    Path(repo_id): Path<String>,
    Json(req): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, AppError> {
    let enabled = req.get("enabled").and_then(|v| v.as_bool()).unwrap_or(true);
    let now = chrono::Utc::now().timestamp();

    let existing = metadata_config::Entity::find()
        .filter(metadata_config::Column::RepoId.eq(&repo_id))
        .one(state.db.as_ref())
        .await?;

    match existing {
        Some(c) => {
            let mut active: metadata_config::ActiveModel = c.into();
            active.enabled = Set(Some(enabled));
            active.update(state.db.as_ref()).await?;
        }
        None => {
            metadata_config::Entity::insert(metadata_config::ActiveModel {
                id: sea_orm::NotSet,
                repo_id: Set(repo_id),
                enabled: Set(Some(enabled)),
                created_at: Set(now),
            })
            .exec(state.db.as_ref())
            .await?;
        }
    }

    Ok(Json(serde_json::json!({"success": true})))
}

/// GET /api/v2.1/repos/{repo_id}/metadata/tags/
pub async fn get_file_tags(
    _auth: AuthUser,
    State(state): State<Arc<AppState>>,
    Path(repo_id): Path<String>,
) -> Result<Json<serde_json::Value>, AppError> {
    let tags = file_tag::Entity::find()
        .filter(file_tag::Column::RepoId.eq(&repo_id))
        .all(state.db.as_ref())
        .await?;

    let tag_list: Vec<String> = tags.into_iter().map(|t| t.tag_name).collect();
    Ok(Json(serde_json::json!({"tags": tag_list})))
}

/// PUT /api/v2.1/repos/{repo_id}/metadata/file-tags/
pub async fn update_file_tags(
    _auth: AuthUser,
    State(state): State<Arc<AppState>>,
    Path(repo_id): Path<String>,
    Json(req): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, AppError> {
    let file_path = req.get("file_path").and_then(|v| v.as_str()).unwrap_or("");
    let tags = req.get("tags").and_then(|v| v.as_array());

    if !file_path.is_empty() {
        // Remove existing tags for this file
        file_tag::Entity::delete_many()
            .filter(file_tag::Column::RepoId.eq(&repo_id))
            .filter(file_tag::Column::FilePath.eq(file_path))
            .exec(state.db.as_ref())
            .await?;

        // Add new tags
        if let Some(tags) = tags {
            let now = chrono::Utc::now().timestamp();
            for tag in tags {
                if let Some(tag_name) = tag.as_str() {
                    file_tag::Entity::insert(file_tag::ActiveModel {
                        id: sea_orm::NotSet,
                        repo_id: Set(repo_id.clone()),
                        file_path: Set(file_path.to_string()),
                        tag_name: Set(tag_name.to_string()),
                        created_at: Set(now),
                    })
                    .exec(state.db.as_ref())
                    .await?;
                }
            }
        }
    }

    Ok(Json(serde_json::json!({"success": true})))
}

/// GET /api/v2.1/repos/{repo_id}/related-users/
pub async fn related_users(
    _auth: AuthUser,
    State(state): State<Arc<AppState>>,
    Path(repo_id): Path<String>,
) -> Result<Json<serde_json::Value>, AppError> {
    // Return users who have access to this repo
    let members = crate::entity::repo_member::Entity::find()
        .filter(crate::entity::repo_member::Column::RepoId.eq(&repo_id))
        .all(state.db.as_ref())
        .await?;

    let users: Vec<String> = members
        .into_iter()
        .filter_map(|m| {
            // Just return user IDs for now
            Some(m.user_id.to_string())
        })
        .collect();

    Ok(Json(serde_json::json!({"users": users})))
}

/// GET /api/v2.1/repos/{repo_id}/custom-share-permissions/
pub async fn custom_share_permissions(
    _auth: crate::auth::middleware::AuthUser,
    _state: axum::extract::State<std::sync::Arc<crate::AppState>>,
    _repo_id: axum::extract::Path<String>,
) -> Result<Json<serde_json::Value>, AppError> {
    Ok(Json(serde_json::json!({"permissions": []})))
}

/// POST /api/v2.1/seadoc/upload-image/{sdoc_uuid}/
pub async fn seadoc_upload_image(
    _auth: crate::auth::middleware::AuthUser,
    _state: axum::extract::State<std::sync::Arc<crate::AppState>>,
    _path: axum::extract::Path<String>,
) -> Result<Json<serde_json::Value>, AppError> {
    Ok(Json(serde_json::json!({"url": ""})))
}

/// GET /api/v2.1/repos/{repo_id}/metadata/record/
pub async fn get_metadata_record(
    _auth: crate::auth::middleware::AuthUser,
    State(state): State<Arc<AppState>>,
    Path(repo_id): Path<String>,
) -> Result<Json<serde_json::Value>, AppError> {
    let records = metadata_record::Entity::find()
        .filter(metadata_record::Column::RepoId.eq(&repo_id))
        .all(state.db.as_ref())
        .await?;

    let items: Vec<serde_json::Value> = records
        .into_iter()
        .map(|r| {
            serde_json::json!({
                "file_path": r.file_path,
                "key": r.record_key,
                "value": r.record_value,
            })
        })
        .collect();

    Ok(Json(serde_json::json!({"records": items})))
}

/// PUT /api/v2.1/repos/{repo_id}/metadata/record/
pub async fn update_metadata_record(
    _auth: crate::auth::middleware::AuthUser,
    State(state): State<Arc<AppState>>,
    Path(repo_id): Path<String>,
    Json(req): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, AppError> {
    let file_path = req.get("file_path").and_then(|v| v.as_str()).unwrap_or("");
    let key = req.get("key").and_then(|v| v.as_str()).unwrap_or("");
    let value = req.get("value").and_then(|v| v.as_str());

    if !file_path.is_empty() && !key.is_empty() {
        let now = chrono::Utc::now().timestamp();

        // Delete existing record for this file+key
        metadata_record::Entity::delete_many()
            .filter(metadata_record::Column::RepoId.eq(&repo_id))
            .filter(metadata_record::Column::FilePath.eq(file_path))
            .filter(metadata_record::Column::RecordKey.eq(key))
            .exec(state.db.as_ref())
            .await?;

        // Insert new record
        metadata_record::Entity::insert(metadata_record::ActiveModel {
            id: sea_orm::NotSet,
            repo_id: sea_orm::Set(repo_id),
            file_path: sea_orm::Set(file_path.to_string()),
            record_key: sea_orm::Set(key.to_string()),
            record_value: sea_orm::Set(value.map(|v| v.to_string())),
            created_at: sea_orm::Set(now),
            updated_at: sea_orm::Set(now),
        })
        .exec(state.db.as_ref())
        .await?;
    }

    Ok(Json(serde_json::json!({"success": true})))
}
