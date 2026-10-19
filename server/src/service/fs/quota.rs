use crate::repository::Repositories;
use base::error::AppError;

/// Read a user's storage usage from a fresh-enough cached snapshot (it lives on
/// `Repositories`, so it describes this server's database), recomputing a full
/// SUM only on cache miss or expiry; commits invalidate it outright, so the TTL
/// is only a backstop.
async fn user_usage(repos: &Repositories, user_id: i32) -> Result<i64, AppError> {
    if let Some(usage) = repos.quota_cache.cached_usage(user_id) {
        return Ok(usage);
    }

    let usage = repos.compute_user_usage(user_id).await?;
    repos.quota_cache.store_usage(user_id, usage);
    Ok(usage)
}

/// Check whether a user has enough remaining quota for a `delta`-byte upload.
///
/// `users.storage_quota` decides: `Some(0)` unlimited, `Some(n)` n bytes, `None`
/// falls back to `default_quota`, the server's default space quota (its `0` is
/// unlimited). Returns `AppError::QuotaExceeded` when the allowance would be
/// exceeded.
pub async fn check_upload_quota(
    repos: &Repositories,
    user_id: i32,
    delta: i64,
    default_quota: u64,
) -> Result<(), AppError> {
    if delta <= 0 {
        return Ok(());
    }

    let Some(quota) = effective_quota(repos, user_id, default_quota).await? else {
        return Ok(());
    };

    // Current usage (sum of owned repo sizes), cached against a full SUM per chunk.
    let usage = user_usage(repos, user_id).await?;

    if usage + delta > quota {
        return Err(AppError::QuotaExceeded);
    }

    Ok(())
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
    let Some(quota) = effective_quota(repos, user_id, default_quota).await? else {
        return Ok(());
    };

    let usage = user_usage(repos, user_id).await?;
    if usage + delta.max(0) >= quota {
        return Err(AppError::QuotaExceeded);
    }

    Ok(())
}

/// Drop a user's cached usage so the next check reads the committed total, else
/// uploads inside the 500 ms window all see pre-commit usage — a check-then-write
/// race that can exceed the quota by roughly the number of concurrent uploads.
/// Commits are the only writer of committed usage.
pub fn invalidate_user(repos: &Repositories, user_id: i32) {
    repos.quota_cache.invalidate_usage(user_id);
}

/// Resolve a user's effective quota in bytes, or `None` for unlimited.
///
/// An account with no quota of its own follows `default_quota`, so changing that
/// setting re-bounds every such account — new registrations included.
async fn effective_quota(
    repos: &Repositories,
    user_id: i32,
    default_quota: u64,
) -> Result<Option<i64>, AppError> {
    let user = repos
        .user
        .find_by_id(user_id)
        .await?
        .ok_or(AppError::Unauthorized)?;

    let quota = match user.storage_quota {
        Some(0) => return Ok(None), // explicitly unlimited
        Some(n) => n,
        None => default_quota as i64, // the account has no quota of its own
    };

    if quota <= 0 {
        return Ok(None); // the default quota is unlimited
    }
    Ok(Some(quota))
}

/// Reserve `delta` bytes of **uncommitted** block writes for `(user_id, repo_id)`.
/// Content-addressed storage means a block write never changes a repo's `size`, so
/// committed usage alone let a client write blocks and never commit without
/// touching quota; a reservation is held until the upload commits or the blocks are
/// deleted. Returns `QuotaExceeded` (HTTP 443) when committed usage plus every
/// outstanding reservation plus `delta` would exceed the quota — callers must not
/// have written the bytes yet, or must clean them up on failure. Reservations live
/// on the server instance, so a restart clears them and GC reclaims the leftovers.
pub async fn reserve_block_bytes(
    repos: &Repositories,
    user_id: i32,
    repo_id: &str,
    delta: i64,
    default_quota: u64,
) -> Result<(), AppError> {
    if delta <= 0 {
        return Ok(());
    }

    let Some(quota) = effective_quota(repos, user_id, default_quota).await? else {
        return Ok(());
    };

    // Committed usage first (cached) plus the reservation; the check and insert
    // share one lock, so concurrent requests cannot both spend the same headroom.
    let usage = user_usage(repos, user_id).await?;
    repos
        .quota_cache
        .reserve_if_fits(user_id, repo_id, delta, usage, quota)
        .map_err(|_| AppError::QuotaExceeded)
}

/// Drop every outstanding reservation for `(user_id, repo_id)`: after a commit those
/// bytes are part of the committed `size` or orphaned (left to GC), so keeping them
/// reserved would double-count; also called when an abandoned upload is deleted.
pub fn release_repo_reservation(repos: &Repositories, user_id: i32, repo_id: &str) {
    let released = repos.quota_cache.release_repo(user_id, repo_id);
    if released != 0 {
        tracing::debug!(
            user_id,
            repo_id,
            bytes = released,
            "released block reservation"
        );
    }
}

// Reservations are unit-tested on `infra::quota_cache::QuotaCache`; quota
// resolution is covered by `server/tests/quota_test.rs`.
