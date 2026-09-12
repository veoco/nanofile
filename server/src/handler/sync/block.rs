use axum::{
    Json, Router,
    extract::{Path, State},
    http::StatusCode,
};
use futures::stream::{self, StreamExt};
use std::sync::Arc;

use crate::AppState;
use crate::handler::{MAX_BLOCK_UPLOAD_BYTES, read_body_limited};
use crate::middleware::auth::SyncAuth;
use base::error::AppError;
use infra::crypto::fs_id::sha1_hex;

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
    auth: SyncAuth,
    Path(repo_id): Path<String>,
    body: axum::body::Body,
) -> Result<Json<Vec<String>>, AppError> {
    // `SyncAuth` only proves the token was issued for this repo; it does not
    // re-check membership, so a member removed while their (year-long) token was
    // still valid kept a block-existence oracle for the library. Every other
    // sync endpoint re-checks, and the client only calls this with write intent
    // (it follows `permission-check?op=upload`), so write permission is the
    // right bar.
    crate::domain::permission::check_repo_write_permission(
        state.repos.member.as_ref(),
        &repo_id,
        auth.user_id,
    )
    .await?;

    let data = axum::body::to_bytes(body, 10 * 1024 * 1024)
        .await
        .map_err(|e| AppError::internal(e.to_string()))?;

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

    // Reject malformed / path-traversal ids before touching the store.
    for id in &block_ids {
        validate_block_id(id)?;
    }

    let block_store = state.block_store.clone();

    let missing: Vec<String> = stream::iter(block_ids)
        .map(move |block_id| {
            let store = block_store.clone();
            let repo_id = repo_id.clone();
            async move {
                if !store.has_block(&repo_id, &block_id).await {
                    Some(block_id)
                } else {
                    None
                }
            }
        })
        .buffered(16)
        .filter_map(|x| async move { x })
        .collect()
        .await;

    Ok(Json(missing))
}

pub async fn get_block(
    State(state): State<Arc<AppState>>,
    auth: SyncAuth,
    Path((repo_id, block_id)): Path<(String, String)>,
) -> Result<Vec<u8>, AppError> {
    validate_block_id(&block_id)?;

    // Defense in depth: the block store is global and content-addressed, so
    // without an explicit membership check a token bound to repo A could read
    // any block whose id it knows (see `fs_id_list` in `sync/fs.rs`).
    crate::domain::permission::check_repo_read_permission(
        state.repos.member.as_ref(),
        &repo_id,
        auth.user_id,
    )
    .await?;

    let block_store = state.block_store.clone();

    block_store
        .read_block(&repo_id, &block_id)
        .await
        .map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                // The block is not part of *this* library (or was pruned):
                // with the per-library layout, "absent" is also the answer for
                // a block id that exists in a library the caller cannot read.
                AppError::NotFound("block not found".into())
            } else {
                AppError::internal(e.to_string())
            }
        })
}

pub async fn put_block(
    State(state): State<Arc<AppState>>,
    auth: SyncAuth,
    Path((repo_id, block_id)): Path<(String, String)>,
    body: axum::body::Body,
) -> Result<StatusCode, AppError> {
    validate_block_id(&block_id)?;

    // Writing a block mutates the repo's object store; a read-only member must
    // not be able to inject data. Checked before the body is buffered.
    crate::domain::permission::check_repo_write_permission(
        state.repos.member.as_ref(),
        &repo_id,
        auth.user_id,
    )
    .await?;

    let data = read_body_limited(body, MAX_BLOCK_UPLOAD_BYTES).await?;

    // Verify the data hash matches the URL block_id.
    // The seaf-daemon sends PUT with block_id = SHA1 of (encrypted) block data.
    let computed = sha1_hex(&data);
    if computed != block_id {
        return Err(AppError::BadRequest(format!(
            "block_id mismatch: expected {} got {}",
            block_id, computed
        )));
    }

    // Charge the block against the user's quota as an **uncommitted** write.
    // Comparing against committed usage alone was ineffective here: content
    // addressing means writing a block never changes any repo's `size`, so
    // every distinct block passed and `put_block` + never-commit accumulated
    // blocks without bound. The reservation is released when the branch update
    // commits the blocks into a file.
    crate::service::fs::quota::reserve_block_bytes(
        &state.repos,
        auth.user_id,
        &repo_id,
        data.len() as i64,
        state.config.storage.max_storage_bytes,
    )
    .await?;

    let block_store = state.block_store.clone();
    block_store
        .write_block_with_id(&repo_id, &block_id, &data)
        .await
        .map_err(|e| AppError::internal(e.to_string()))?;

    Ok(StatusCode::OK)
}

pub async fn get_block_map(
    State(state): State<Arc<AppState>>,
    auth: SyncAuth,
    Path((repo_id, file_id)): Path<(String, String)>,
) -> Result<Json<Vec<i64>>, AppError> {
    // Membership check (see `get_block`).
    crate::domain::permission::check_repo_read_permission(
        state.repos.member.as_ref(),
        &repo_id,
        auth.user_id,
    )
    .await?;

    let fs_obj = state
        .repos
        .fs_object
        .find_by_repo_and_fs_id(&repo_id, &file_id)
        .await?
        .ok_or_else(|| AppError::NotFound("file not found".into()))?;

    let json_val: serde_json::Value =
        serde_json::from_str(&fs_obj.data).map_err(|e| AppError::internal(e.to_string()))?;

    let block_ids = json_val
        .get("block_ids")
        .and_then(|v| v.as_array())
        .ok_or_else(|| AppError::internal("invalid file object"))?;

    let block_store = state.block_store.clone();

    // Extract block IDs as strings for concurrent processing
    let block_id_strs: Vec<String> = block_ids
        .iter()
        .filter_map(|v| v.as_str().map(|s| s.to_string()))
        .collect();

    let block_sizes: Vec<i64> = stream::iter(block_id_strs)
        .map(move |bid| {
            let store = block_store.clone();
            let repo_id = repo_id.clone();
            async move { store.block_size(&repo_id, &bid).await.unwrap_or(0) }
        })
        .buffered(16)
        .collect()
        .await;

    Ok(Json(block_sizes))
}
