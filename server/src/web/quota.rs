use crate::error::AppError;
use crate::repository::Repositories;

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

    // Compute current usage (sum of owned repo sizes).
    let owned_repos = repos.repo.find_by_owner_id(user_id).await?;
    let usage: i64 = owned_repos.iter().map(|r| r.size).sum();

    if usage + delta > quota {
        return Err(AppError::QuotaExceeded);
    }

    Ok(())
}
