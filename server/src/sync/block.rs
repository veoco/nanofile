use axum::{
    Json, Router,
    extract::{Path, State},
    http::StatusCode,
};
use futures::stream::{self, StreamExt};
use std::sync::Arc;

use crate::AppState;
use crate::auth::middleware::SyncAuth;
use crate::crypto::fs_id::sha1_hex;
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

/// Validate that a block_id is exactly 40 lowercase hex characters.
/// Matches seafile-server's is_object_id_valid() behavior.
fn validate_block_id(block_id: &str) -> Result<(), AppError> {
    if block_id.len() != 40
        || !block_id
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
    {
        return Err(AppError::BadRequest(format!(
            "invalid block_id format: {}",
            block_id
        )));
    }
    Ok(())
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

    let missing: Vec<String> = stream::iter(block_ids)
        .map(move |block_id| {
            let store = block_store.clone();
            async move {
                if !store.has_block(&block_id).await {
                    Some(block_id)
                } else {
                    None
                }
            }
        })
        .buffered(8)
        .filter_map(|x| async move { x })
        .collect()
        .await;

    Ok(Json(missing))
}

pub async fn get_block(
    State(state): State<Arc<AppState>>,
    _auth: SyncAuth,
    Path((_repo_id, block_id)): Path<(String, String)>,
) -> Result<Vec<u8>, AppError> {
    validate_block_id(&block_id)?;

    let block_store = state.block_store.clone();

    block_store
        .read_block(&block_id)
        .await
        .map_err(|e| AppError::Internal(e.to_string()))
}

pub async fn put_block(
    State(state): State<Arc<AppState>>,
    _auth: SyncAuth,
    Path((_repo_id, block_id)): Path<(String, String)>,
    body: axum::body::Body,
) -> Result<StatusCode, AppError> {
    validate_block_id(&block_id)?;

    let max_bytes = (state.config.server.max_upload_size_mb * 1024 * 1024) as usize;
    let data = axum::body::to_bytes(body, max_bytes)
        .await
        .map_err(|e| AppError::Internal(e.to_string()))?;

    // Verify the data hash matches the URL block_id.
    // The seaf-daemon sends PUT with block_id = SHA1 of (encrypted) block data.
    let computed = sha1_hex(&data);
    if computed != block_id {
        return Err(AppError::BadRequest(format!(
            "block_id mismatch: expected {} got {}",
            block_id, computed
        )));
    }

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
    let fs_obj = state
        .repos
        .fs_object
        .find_by_repo_and_fs_id(&repo_id, &file_id)
        .await?
        .ok_or_else(|| AppError::NotFound("file not found".into()))?;

    let json_val: serde_json::Value =
        serde_json::from_str(&fs_obj.data).map_err(|e| AppError::Internal(e.to_string()))?;

    let block_ids = json_val
        .get("block_ids")
        .and_then(|v| v.as_array())
        .ok_or_else(|| AppError::Internal("invalid file object".into()))?;

    let block_store = state.block_store.clone();

    // Extract block IDs as strings for concurrent processing
    let block_id_strs: Vec<String> = block_ids
        .iter()
        .filter_map(|v| v.as_str().map(|s| s.to_string()))
        .collect();

    let block_sizes: Vec<i64> = stream::iter(block_id_strs)
        .map(move |bid| {
            let store = block_store.clone();
            let block_dir = state.block_dir.clone();
            async move {
                let path = block_dir
                    .as_path()
                    .join(&bid[..bid.len().min(2)])
                    .join(&bid);
                if store.has_block(&bid).await {
                    tokio::fs::metadata(&path)
                        .await
                        .map(|m| m.len() as i64)
                        .unwrap_or(0)
                } else {
                    0
                }
            }
        })
        .buffered(8)
        .collect()
        .await;

    Ok(Json(block_sizes))
}
