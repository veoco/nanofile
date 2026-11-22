use axum::{
    Json, Router,
    extract::{Path, State},
    http::StatusCode,
};
use futures::stream::{self, StreamExt};
use std::sync::Arc;

use crate::AppState;
use crate::fs::core::upload_activity;
use crate::handler::{MAX_BLOCK_UPLOAD_BYTES, read_body_limited};
use crate::middleware::auth::{SyncAuth, SyncRepoRead};
use base::error::AppError;

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

/// Validate that block_id is 40 lowercase hex chars (seafile's is_object_id_valid()).
fn validate_block_id(block_id: &str) -> Result<(), AppError> {
    if !crate::handler::is_valid_block_id(block_id) {
        return Err(AppError::BadRequest(format!(
            "invalid block_id format: {}",
            block_id
        )));
    }
    Ok(())
}

pub async fn check_blocks(
    State(state): State<Arc<AppState>>,
    _auth: SyncRepoRead,
    Path(repo_id): Path<String>,
    body: axum::body::Body,
) -> Result<Json<Vec<String>>, AppError> {
    let data = axum::body::to_bytes(body, 10 * 1024 * 1024)
        .await
        .map_err(crate::handler::body_read_error)?;

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

    let missing =
        crate::handler::block::missing_blocks(state.block_store.clone(), &repo_id, block_ids).await;

    Ok(Json(missing))
}

pub async fn get_block(
    State(state): State<Arc<AppState>>,
    _auth: SyncRepoRead,
    Path((repo_id, block_id)): Path<(String, String)>,
) -> Result<Vec<u8>, AppError> {
    validate_block_id(&block_id)?;

    let block_store = state.block_store.clone();

    block_store
        .read_block(&repo_id, &block_id)
        .await
        .map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                // Not part of *this* library (or pruned); per-library layout, so
                // "absent" also answers for a block that exists elsewhere.
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

    // Writing a block mutates the object store; checked before the body is buffered.
    // The same row names the owner the quota is charged to, so the reservation
    // below does not look it up again for every block.
    let owner = crate::domain::permission::repo_write_owner(
        state.repos.member.as_ref(),
        &repo_id,
        auth.user_id,
    )
    .await?;

    // A block upload is part of a multi-request sync; keep GC out of the repo
    // until the branch update arrives (or the lease expires).
    upload_activity::touch(&repo_id);

    let data = read_body_limited(body, MAX_BLOCK_UPLOAD_BYTES).await?;

    // Verify the hash and store the block, charging the owner's quota as an
    // **uncommitted** write: content addressing means writing a block never
    // changes any repo's `size`, so comparing committed usage alone let
    // `put_block` + never-commit accumulate blocks without bound. The reservation
    // is released when the branch update commits the blocks.
    crate::handler::block::write_client_block(&state, &repo_id, owner, &block_id, &data).await?;

    Ok(StatusCode::OK)
}

pub async fn get_block_map(
    State(state): State<Arc<AppState>>,
    _auth: SyncRepoRead,
    Path((repo_id, file_id)): Path<(String, String)>,
) -> Result<Json<Vec<i64>>, AppError> {
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

    let block_id_strs: Vec<String> = block_ids
        .iter()
        .filter_map(|v| v.as_str().map(|s| s.to_string()))
        .collect();

    let block_sizes: Vec<i64> = {
        let sizes = stream::iter(block_id_strs)
            .map(move |bid| {
                let store = block_store.clone();
                let repo_id = repo_id.clone();
                async move { store.block_size(&repo_id, &bid).await }
            })
            .buffered(16)
            .collect::<Vec<_>>()
            .await;
        // A missing block is an error, not a zero-length one: upstream answers 500
        // (`sync_api.go:303-307`), and 0 makes a seadrive client compute wrong offsets.
        let mut out = Vec::with_capacity(sizes.len());
        for size in sizes {
            out.push(
                size.map_err(|e| AppError::Internal(format!("block size lookup failed: {e}")))?,
            );
        }
        out
    };

    // A file object whose blocks do not add up to its declared size cannot be mapped
    // onto offsets: a client slicing its download from these sizes would drop or
    // duplicate bytes at the seam.
    if let Some(declared) = json_val.get("size").and_then(|v| v.as_i64()) {
        let actual: i64 = block_sizes.iter().sum();
        if actual != declared {
            return Err(AppError::Internal(format!(
                "file object {file_id} declares {declared} bytes but its {} blocks hold {actual}",
                block_sizes.len()
            )));
        }
    }

    Ok(Json(block_sizes))
}
