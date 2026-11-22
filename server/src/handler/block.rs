//! Block-level helpers shared by the sync and web upload endpoints.

use crate::AppState;
use base::error::AppError;
use infra::crypto::fs_id::sha1_hex;

/// Verify and store one client-supplied block, charging the owner's quota.
///
/// The id must equal `sha1(data)`. A new block keeps the reservation taken here;
/// a block that already exists (a retry or a concurrent upload) is already paid
/// for, so its reservation is released. Returns whether the block was created.
pub async fn write_client_block(
    state: &AppState,
    repo_id: &str,
    owner: i32,
    block_id: &str,
    data: &[u8],
) -> Result<bool, AppError> {
    let computed = sha1_hex(data);
    if computed != block_id {
        return Err(AppError::BadRequest(format!(
            "block_id mismatch: expected {block_id} got {computed}"
        )));
    }

    // Charge the block before writing it, so a client that only uploads blocks
    // (never a commit) cannot fill the disk while committed usage stays at zero.
    // 443 is what the Qt client and seaf-daemon map to "out of quota".
    crate::service::fs::quota::reserve_block_bytes_for_owner(
        &state.repos,
        owner,
        repo_id,
        block_id,
        data.len() as i64,
        state.config().storage.default_user_quota_bytes,
    )
    .await?;

    match state
        .block_store
        .write_block_with_id_tracked(repo_id, block_id, data)
        .await
    {
        Ok((_, true)) => Ok(true),
        Ok((_, false)) => {
            crate::service::fs::quota::release_block_bytes(&state.repos, repo_id, block_id).await;
            Ok(false)
        }
        Err(e) => {
            crate::service::fs::quota::release_block_bytes(&state.repos, repo_id, block_id).await;
            Err(AppError::internal(e.to_string()))
        }
    }
}

/// Which of `block_ids` the library does not have yet, in request order.
///
/// The ids must already be validated: they are only used as store keys here.
pub async fn missing_blocks(
    store: infra::storage::DynBlockStorage,
    repo_id: &str,
    block_ids: Vec<String>,
) -> Vec<String> {
    use futures::StreamExt;

    let repo_id = repo_id.to_string();
    futures::stream::iter(block_ids)
        .map(move |block_id| {
            let store = store.clone();
            let repo_id = repo_id.clone();
            async move {
                if store.has_block(&repo_id, &block_id).await {
                    None
                } else {
                    Some(block_id)
                }
            }
        })
        .buffered(16)
        .filter_map(|missing| async move { missing })
        .collect()
        .await
}
