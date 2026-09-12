use crate::repository::Repositories;
use base::error::AppError;

/// Read a user's storage usage, serving a fresh-enough cached snapshot if
/// available and recomputing (full SUM) only on cache miss or expiry.
///
/// The snapshot lives on the `Repositories` instance (`infra::quota_cache`), so
/// it always describes the database this server is using: `repo.size` only
/// changes on commit, which is a low-frequency path compared to the per-chunk
/// checks that read this cache. Commits invalidate the entry outright
/// ([`invalidate_user`]), so the TTL is only a backstop.
async fn user_usage(repos: &Repositories, user_id: i32) -> Result<i64, AppError> {
    if let Some(usage) = repos.quota_cache.cached_usage(user_id) {
        return Ok(usage);
    }

    let usage = repos.compute_user_usage(user_id).await?;
    repos.quota_cache.store_usage(user_id, usage);
    Ok(usage)
}

/// Check whether a user has enough remaining storage quota for an upload of
/// `delta` bytes.
///
/// The effective quota is determined by:
/// - `users.storage_quota`: `Some(0)` = unlimited,
///   `Some(n)` = n bytes, `None` = fall back to `global_max`.
/// - `global_max` (from `config.storage.max_storage_bytes`):
///   `0` = unlimited, otherwise the global cap.
///
/// Returns `Ok(())` on success or `AppError::QuotaExceeded` when the user
/// would exceed their allowance.
pub async fn check_upload_quota(
    repos: &Repositories,
    user_id: i32,
    delta: i64,
    global_max: u64,
) -> Result<(), AppError> {
    if delta <= 0 {
        return Ok(());
    }

    let Some(quota) = effective_quota(repos, user_id, global_max).await? else {
        return Ok(());
    };

    // Current usage (sum of owned repo sizes), served from a short-TTL cache
    // to avoid a full-table SUM on every put_block / upload chunk.
    let usage = user_usage(repos, user_id).await?;

    if usage + delta > quota {
        return Err(AppError::QuotaExceeded);
    }

    Ok(())
}

/// Drop a user's cached usage so the next check reads the committed total.
///
/// Without this, a commit leaves the 500 ms snapshot in place and several
/// uploads started inside that window all see the pre-commit usage — a
/// check-then-write race that lets the aggregate exceed the quota by roughly
/// the number of concurrent uploads. Commits are the only path that changes
/// committed usage, so invalidating there is enough.
pub fn invalidate_user(repos: &Repositories, user_id: i32) {
    repos.quota_cache.invalidate_usage(user_id);
}

/// Resolve a user's effective quota in bytes, or `None` for unlimited.
async fn effective_quota(
    repos: &Repositories,
    user_id: i32,
    global_max: u64,
) -> Result<Option<i64>, AppError> {
    let user = repos
        .user
        .find_by_id(user_id)
        .await?
        .ok_or(AppError::Unauthorized)?;

    let quota = match user.storage_quota {
        Some(0) => return Ok(None), // explicitly unlimited
        Some(n) => n,
        None => global_max as i64, // fall back to global
    };

    if quota <= 0 {
        return Ok(None); // global unlimited
    }
    Ok(Some(quota))
}

/// Reserve `delta` bytes of **uncommitted** block writes for
/// `(user_id, repo_id)`.
///
/// Content-addressed storage means writing a block never changes any repo's
/// `size`, so comparing against committed usage alone let a client write blocks
/// and never commit without ever touching a quota. The reservation closes that:
/// it is charged until the upload commits (which folds the bytes into the
/// committed total) or the blocks are deleted again.
///
/// Returns `QuotaExceeded` (HTTP 443) when committed usage plus every
/// outstanding reservation plus `delta` would exceed the user's quota. Callers
/// must not have written the bytes yet, or must clean them up when this fails.
///
/// Reservations live on the server instance, so a restart clears them; blocks
/// left behind are reclaimed by GC, which is why quota is enforced on the bytes
/// their writer can still reach rather than on disk occupancy.
pub async fn reserve_block_bytes(
    repos: &Repositories,
    user_id: i32,
    repo_id: &str,
    delta: i64,
    global_max: u64,
) -> Result<(), AppError> {
    if delta <= 0 {
        return Ok(());
    }

    let Some(quota) = effective_quota(repos, user_id, global_max).await? else {
        return Ok(());
    };

    // Committed usage first (cached); it never includes uncommitted blocks, so
    // the reservation is what closes the gap. The check and the insert share
    // one lock, so concurrent requests cannot both spend the same headroom.
    let usage = user_usage(repos, user_id).await?;
    repos
        .quota_cache
        .reserve_if_fits(user_id, repo_id, delta, usage, quota)
        .map_err(|_| AppError::QuotaExceeded)
}

/// Drop every outstanding reservation for `(user_id, repo_id)`.
///
/// Called when an upload commits: the bytes that were reserved are now either
/// part of the repo's committed `size` (counted by `user_usage`) or genuinely
/// orphaned (left to GC), so keeping them reserved would double-count. Also
/// called when an abandoned upload's blocks are deleted.
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

// Reservation behaviour is unit-tested on `infra::quota_cache::QuotaCache`
// (see that module), because that is where the account state lives now. The
// quota-resolution semantics are covered end-to-end by
// `server/tests/quota_test.rs`.
