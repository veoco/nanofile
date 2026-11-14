use crate::repository::Repositories;
use base::error::AppError;

/// Read a user's storage usage from a fresh-enough cached snapshot (it lives on
/// `Repositories`, so it describes this server's database), recomputing a full
/// SUM only on cache miss or expiry; commits invalidate it outright, so the TTL
/// is only a backstop. Prechecks use this; authoritative commit checks use
/// `Repositories::check_repo_quota_locked`, which sums under the per-user lock.
async fn user_usage(repos: &Repositories, user_id: i32) -> Result<i64, AppError> {
    if let Some(usage) = repos.quota_cache.cached_usage(user_id) {
        return Ok(usage);
    }

    let usage = repos.compute_user_usage(user_id).await?;
    repos.quota_cache.store_usage(user_id, usage);
    Ok(usage)
}

/// Sync-protocol quota probe (`GET /seafhttp/repo/{id}/quota-check/?delta=`):
/// upstream's `checkQuota` (fileserver/quota.go) tests `usage + delta >= quota`
/// for *any* delta, so a full client gets "out of quota", not a 200.
pub async fn check_sync_quota(
    repos: &Repositories,
    user_id: i32,
    delta: i64,
    default_quota: u64,
) -> Result<(), AppError> {
    let Some(quota) = repos.effective_user_quota(user_id, default_quota).await? else {
        return Ok(());
    };

    let usage = user_usage(repos, user_id).await?;
    if usage + delta.max(0) >= quota {
        return Err(AppError::QuotaExceeded);
    }

    Ok(())
}

/// Reserve one **new** block write against the repository owner's quota.
///
/// The block id makes the reservation idempotent: a client that re-sends the same
/// block, or retries a `PUT block` after a timeout, does not reserve its bytes
/// twice. Callers must release it when the block is found to be already present
/// (`was_new == false`) or when the upload commits/aborts.
pub async fn reserve_block_bytes(
    repos: &Repositories,
    repo_id: &str,
    block_id: &str,
    delta: i64,
    default_quota: u64,
) -> Result<(), AppError> {
    if delta <= 0 {
        return Ok(());
    }

    // Quota is a property of the repository owner, not of whoever happens to be
    // holding a sync token. Current libraries are single-user, but keeping the
    // owner lookup here means a future share cannot bill the wrong account.
    let owner = repos.repo_owner_id(repo_id).await?;
    reserve_block_bytes_for_owner(repos, owner, repo_id, block_id, delta, default_quota).await
}

/// [`reserve_block_bytes`] for a caller that already knows the owner.
///
/// The sync block paths read the owner off the write-permission row, so a per-block
/// `repo_owner_id` lookup would be a second round trip per block for the same row.
pub async fn reserve_block_bytes_for_owner(
    repos: &Repositories,
    owner: i32,
    repo_id: &str,
    block_id: &str,
    delta: i64,
    default_quota: u64,
) -> Result<(), AppError> {
    if delta <= 0 {
        return Ok(());
    }

    let scope = repos.lock_quota(owner).await;

    let Some(quota) = repos.effective_user_quota(owner, default_quota).await? else {
        return Ok(());
    };

    // The cached total is safe under the scope: every size-changing commit holds
    // the same scope and invalidates the entry before releasing it.
    let usage = user_usage(repos, owner).await?;
    repos
        .quota_cache
        .reserve_block(owner, repo_id, block_id, delta, usage, quota)
        .map_err(|_| AppError::QuotaExceeded)?;
    drop(scope);
    Ok(())
}

/// Reserve a resumable upload's declared size against the repository owner's
/// quota. The upload id makes a retried first chunk idempotent.
pub async fn reserve_upload_bytes(
    repos: &Repositories,
    repo_id: &str,
    upload_id: &str,
    delta: i64,
    default_quota: u64,
) -> Result<(), AppError> {
    if delta <= 0 {
        return Ok(());
    }
    let owner = repos.repo_owner_id(repo_id).await?;
    let scope = repos.lock_quota(owner).await;
    let Some(quota) = repos.effective_user_quota(owner, default_quota).await? else {
        return Ok(());
    };
    let usage = user_usage(repos, owner).await?;
    repos
        .quota_cache
        .reserve_upload(owner, repo_id, upload_id, delta, usage, quota)
        .map_err(|_| AppError::QuotaExceeded)?;
    drop(scope);
    Ok(())
}

/// Drop one block reservation (deduped write, failed write, or committed block).
pub async fn release_block_bytes(repos: &Repositories, repo_id: &str, block_id: &str) -> i64 {
    let owner = match repos.repo_owner_id(repo_id).await {
        Ok(owner) => owner,
        Err(_) => return 0,
    };
    repos.quota_cache.release_block(owner, repo_id, block_id)
}

/// Drop several block reservations after a successful commit.
pub async fn release_blocks(repos: &Repositories, repo_id: &str, block_ids: &[String]) -> i64 {
    if block_ids.is_empty() {
        return 0;
    }
    let owner = match repos.repo_owner_id(repo_id).await {
        Ok(owner) => owner,
        Err(_) => return 0,
    };
    repos.quota_cache.release_blocks(owner, repo_id, block_ids)
}

/// Drop one resumable upload's reservation. `owner` comes from the temp-upload
/// entry, so this still works while the repository row is being deleted.
pub fn release_upload(repos: &Repositories, owner: i32, repo_id: &str, upload_id: &str) -> i64 {
    repos.quota_cache.release_upload(owner, repo_id, upload_id)
}

// Authoritative quota resolution and the check-and-commit sequence live on
// `Repositories` (`effective_user_quota`, `lock_quota`,
// `check_repo_quota_locked`); the unit tests for reservations are on
// `infra::quota_cache::QuotaCache` and the integration tests on
// `server/tests/quota_test.rs`.
