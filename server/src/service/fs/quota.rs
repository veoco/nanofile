use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use crate::repository::Repositories;
use base::error::AppError;

/// Per-user storage-usage cache so high-frequency quota checks (every
/// `put_block` / upload chunk) don't run a full `SUM(repo.size)` scan on the
/// `repo` table each time.
///
/// Safety: `repo.size` only changes on commit (`adjust_repo_size`), which is a
/// low-frequency path. The `put_block`/chunk paths that hit this cache the most
/// never change usage, so returning a brief TTL'd snapshot is correct. The short
/// TTL keeps the commit path's accounting eventually-consistent.
const USAGE_CACHE_TTL: Duration = Duration::from_millis(500);
/// Hard cap so the map can't grow without bound on many distinct users.
const USAGE_CACHE_MAX: usize = 4096;

static USAGE_CACHE: OnceLock<Mutex<HashMap<i32, (i64, Instant)>>> = OnceLock::new();

fn usage_cache() -> &'static Mutex<HashMap<i32, (i64, Instant)>> {
    USAGE_CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Read a user's storage usage, serving a fresh-enough cached snapshot if
/// available and recomputing (full SUM) only on cache miss or expiry.
async fn user_usage(repos: &Repositories, user_id: i32) -> Result<i64, AppError> {
    let now = Instant::now();
    {
        let map = usage_cache().lock().unwrap_or_else(|e| e.into_inner());
        if let Some((usage, at)) = map.get(&user_id)
            && now.duration_since(*at) < USAGE_CACHE_TTL
        {
            return Ok(*usage);
        }
    }

    let usage = repos.compute_user_usage(user_id).await?;

    {
        let mut map = usage_cache().lock().unwrap_or_else(|e| e.into_inner());
        // Bound growth: drop stale entries first; if still at capacity, skip
        // caching this result and fall back to real-time next time.
        if map.len() >= USAGE_CACHE_MAX {
            map.retain(|_, (_, at)| now.duration_since(*at) < USAGE_CACHE_TTL);
        }
        if map.len() < USAGE_CACHE_MAX {
            map.insert(user_id, (usage, now));
        }
    }
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

    let user = repos
        .user
        .find_by_id(user_id)
        .await?
        .ok_or(AppError::Unauthorized)?;

    // Resolve effective quota.
    let quota: i64 = match user.storage_quota {
        Some(0) => return Ok(()), // explicitly unlimited
        Some(n) => n,
        None => global_max as i64, // fall back to global
    };

    if quota <= 0 {
        return Ok(()); // global unlimited
    }

    // Current usage (sum of owned repo sizes), served from a short-TTL cache
    // to avoid a full-table SUM on every put_block / upload chunk.
    let usage = user_usage(repos, user_id).await?;

    if usage + delta > quota {
        return Err(AppError::QuotaExceeded);
    }

    Ok(())
}
