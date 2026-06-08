use axum::{
    Json, Router,
    extract::{Path, State},
    http::StatusCode,
};
use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};
use std::sync::Arc;

use crate::AppState;
use crate::auth::middleware::SyncAuth;
use crate::entity::fs_object;
use crate::error::AppError;

pub fn block_routes() -> Router<Arc<AppState>> {
    Router::new()
        .route(
            "/{repo_id}/check-blocks/",
            axum::routing::post(check_blocks),
        )
        .route(
            "/{repo_id}/block/{block_id}",
            axum::routing::get(get_block).put(put_block),
        )
        .route(
            "/{repo_id}/block-map/{file_id}",
            axum::routing::get(get_block_map),
        )
}

pub async fn check_blocks(
    State(state): State<Arc<AppState>>,
    _auth: SyncAuth,
    Path(_repo_id): Path<String>,
    body: axum::body::Body,
) -> Result<Json<Vec<String>>, AppError> {
    let data = axum::body::to_bytes(body, 10 * 1024 * 1024)
        .await
        .map_err(|e| AppError::Internal(e.to_string()))?;

    // Try JSON array first, then fall back to URL-encoded form
    let block_ids: Vec<String> = if let Ok(arr) = serde_json::from_slice::<Vec<String>>(&data) {
        arr
    } else {
        let body_str = String::from_utf8_lossy(&data);
        let mut ids = Vec::new();
        for pair in body_str.split('&') {
            let mut parts = pair.splitn(2, '=');
            let key = parts.next().unwrap_or("");
            let value = parts.next().unwrap_or("");
            if key == "block_ids" && !value.is_empty() {
                ids.push(value.to_string());
            }
        }
        ids
    };

    let block_store = state.block_store.clone();

    let mut missing = Vec::new();
    for block_id in &block_ids {
        if !block_store.has_block(block_id).await {
            missing.push(block_id.clone());
        }
    }

    Ok(Json(missing))
}

pub async fn get_block(
    State(state): State<Arc<AppState>>,
    _auth: SyncAuth,
    Path((_repo_id, block_id)): Path<(String, String)>,
) -> Result<Vec<u8>, AppError> {
    let block_store = state.block_store.clone();

    block_store
        .read_block(&block_id)
        .await
        .map_err(|e| AppError::Internal(e.to_string()))
}

pub async fn put_block(
    State(state): State<Arc<AppState>>,
    _auth: SyncAuth,
    Path((_repo_id, _block_id)): Path<(String, String)>,
    body: axum::body::Body,
) -> Result<StatusCode, AppError> {
    let data = axum::body::to_bytes(body, usize::MAX)
        .await
        .map_err(|e| AppError::Internal(e.to_string()))?;

    let block_store = state.block_store.clone();

    block_store
        .write_block(&data)
        .await
        .map_err(|e| AppError::Internal(e.to_string()))?;

    Ok(StatusCode::OK)
}

pub async fn get_block_map(
    State(state): State<Arc<AppState>>,
    _auth: SyncAuth,
    Path((repo_id, file_id)): Path<(String, String)>,
) -> Result<Json<Vec<i64>>, AppError> {
    let fs_obj = fs_object::Entity::find()
        .filter(fs_object::Column::RepoId.eq(&repo_id))
        .filter(fs_object::Column::FsId.eq(&file_id))
        .one(state.db.as_ref())
        .await?
        .ok_or_else(|| AppError::NotFound("file not found".into()))?;

    let json_val: serde_json::Value =
        serde_json::from_str(&fs_obj.data).map_err(|e| AppError::Internal(e.to_string()))?;

    let block_ids = json_val
        .get("block_ids")
        .and_then(|v| v.as_array())
        .ok_or_else(|| AppError::Internal("invalid file object".into()))?;

    let block_store = state.block_store.clone();

    let mut block_sizes = Vec::new();
    for bid_val in block_ids {
        if let Some(bid) = bid_val.as_str() {
            let path = state
                .block_dir
                .as_path()
                .join(&bid[..bid.len().min(2)])
                .join(bid);
            let size = if block_store.has_block(bid).await {
                std::fs::metadata(&path)
                    .map(|m| m.len() as i64)
                    .unwrap_or(0)
            } else {
                0
            };
            block_sizes.push(size);
        }
    }

    Ok(Json(block_sizes))
}
