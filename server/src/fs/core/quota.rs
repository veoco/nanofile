//! The quota lock shared by every sized commit.
//!
//! Quota checks and the commit that spends the checked headroom must run under
//! one per-user lock; keeping the guard in the filesystem layer lets both the
//! service write paths and `fs`'s own restore path share it without either one
//! reaching into the other.

use crate::repository::Repositories;
use base::error::AppError;

/// Holds a repository owner's quota lock across a sized commit.
///
/// Every write path used to spell out "owner lookup, lock, read the default
/// quota" itself, and one that forgot part of it could either overshoot the quota
/// or hold the lock across unrelated work. Drop the guard as soon as the commit
/// returns.
pub(crate) struct QuotaGuard {
    scope: infra::quota_cache::QuotaScope,
    default_quota: u64,
}

impl QuotaGuard {
    pub(crate) async fn begin(
        repos: &Repositories,
        repo_id: &str,
        default_quota: u64,
    ) -> Result<Self, AppError> {
        let owner = repos.repo_owner_id(repo_id).await?;
        let scope = repos.lock_quota(owner).await;
        Ok(Self {
            scope,
            default_quota,
        })
    }

    pub(crate) fn scope(&self) -> &infra::quota_cache::QuotaScope {
        &self.scope
    }

    pub(crate) fn default_quota(&self) -> u64 {
        self.default_quota
    }
}
