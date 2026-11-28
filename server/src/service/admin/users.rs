use std::sync::Arc;

use crate::repository::Repositories;
use base::error::AppError;

#[derive(serde::Serialize)]
pub struct UserAdminInfo {
    pub id: i32,
    pub email: String,
    /// Display name (`display_name` → `name` → email local part), used by the
    /// seahub-shaped admin list.
    pub name: String,
    pub is_active: bool,
    pub is_admin: bool,
    pub email_verified: bool,
    pub storage_quota: Option<i64>,
    pub usage: i64,
    pub created_at: i64,
    pub last_login_at: Option<i64>,
}

pub struct AdminUserService {
    repos: Arc<Repositories>,
    /// Present when the service is built from `AppState`; used to drop the
    /// account's outstanding capability URLs when it is deactivated.
    token_manager: Option<Arc<crate::service::auth::access_token::AccessTokenManager>>,
}

impl AdminUserService {
    pub fn new(repos: Arc<Repositories>) -> Self {
        Self {
            repos,
            token_manager: None,
        }
    }

    pub fn with_token_manager(
        mut self,
        token_manager: Arc<crate::service::auth::access_token::AccessTokenManager>,
    ) -> Self {
        self.token_manager = Some(token_manager);
        self
    }

    pub async fn list_users(&self) -> Result<Vec<UserAdminInfo>, AppError> {
        let users = self.repos.user.find_all().await?;

        // One grouped SUM query for all owners instead of one per user.
        let user_ids: Vec<i32> = users.iter().map(|u| u.id).collect();
        let usage_by_owner: std::collections::HashMap<i32, i64> = self
            .repos
            .repo
            .sum_sizes_by_owner(&user_ids)
            .await?
            .into_iter()
            .collect();

        let mut result = Vec::with_capacity(users.len());
        for u in users {
            let usage = usage_by_owner.get(&u.id).copied().unwrap_or(0);
            let name = u.nickname();
            result.push(UserAdminInfo {
                id: u.id,
                email: u.email,
                name,
                is_active: u.is_active,
                is_admin: u.is_admin,
                email_verified: u.email_verified,
                storage_quota: u.storage_quota,
                usage,
                created_at: u.created_at,
                last_login_at: u.last_login_at,
            });
        }

        Ok(result)
    }

    pub async fn compute_usage(&self, user_id: i32) -> Result<i64, AppError> {
        self.repos.compute_user_usage(user_id).await
    }

    /// Create a new user (password should already be hashed).
    pub async fn create_user(
        &self,
        email: String,
        password_hash: String,
        is_admin: bool,
        is_active: bool,
        storage_quota: Option<i64>,
    ) -> Result<(), AppError> {
        if self.repos.user.exists_by_email(&email).await? {
            return Err(AppError::BadRequest("user already exists".into()));
        }
        self.repos
            .user
            .create_with_params(email, password_hash, is_admin, is_active, storage_quota)
            .await?;
        Ok(())
    }

    pub async fn update_user(
        &self,
        user_id: i32,
        is_admin: bool,
        is_active: bool,
        storage_quota: Option<i64>,
    ) -> Result<(), AppError> {
        let old = self
            .repos
            .user
            .find_by_id(user_id)
            .await?
            .ok_or_else(|| AppError::NotFound("user not found".into()))?;

        // A shared/public library is managed by its owner, and only an
        // administrator can manage one. Demoting the owner would leave the
        // library nobody can administer, so require the administrator to revoke
        // the sharing first.
        if old.is_admin && !is_admin && self.owns_shared_libraries(user_id).await? {
            return Err(AppError::BadRequest(
                "revoke the library sharing before demoting this account".into(),
            ));
        }

        // Deactivating an account is a revocation, not just a flag: keeping the
        // rows would let every old session, sync token and API key work again as
        // soon as the account is re-enabled, and an unused reset link would keep
        // offering account takeover for the rest of its TTL.
        let was_active = Some(old.is_active);

        self.repos.user.update_is_admin(user_id, is_admin).await?;
        self.repos.user.update_is_active(user_id, is_active).await?;
        self.repos
            .user
            .update_storage_quota(user_id, storage_quota)
            .await?;
        // Deactivating an account must revoke the anonymous links it created immediately.
        crate::service::sharing::share::invalidate_link_creator_cache(user_id, None);
        // The token-only endpoints (`/blks/`, `/upload-blks-api/`) authenticate
        // with an in-memory capability rather than a session; without this an
        // already-minted URL would keep working for its remaining TTL.
        if !is_active && let Some(manager) = &self.token_manager {
            manager.revoke_user(user_id);
        }
        if !is_active && was_active != Some(false) {
            crate::service::auth::token::revoke_all_credentials(
                &self.repos,
                self.token_manager.as_ref(),
                user_id,
                None,
            )
            .await?;
        }
        Ok(())
    }

    pub async fn delete_user(&self, user_id: i32) -> Result<(), AppError> {
        // Deleting the owner of a shared/public library would cascade the rows
        // away and silently revoke every recipient. Require the administrator to
        // revoke the sharing first, which also makes the intent explicit.
        if self.owns_shared_libraries(user_id).await? {
            return Err(AppError::BadRequest(
                "revoke the library sharing before deleting this account".into(),
            ));
        }
        // Drop cached link-authorization decisions before this account's rows disappear.
        crate::service::sharing::share::invalidate_link_creator_cache(user_id, None);
        self.repos.user.delete_user(user_id).await?;
        Ok(())
    }

    /// Whether the account owns at least one library whose visibility is not
    /// `private`.
    async fn owns_shared_libraries(&self, user_id: i32) -> Result<bool, AppError> {
        Ok(self
            .repos
            .repo
            .find_by_owner_id(user_id)
            .await?
            .iter()
            .any(|repo| {
                repo.visibility != crate::domain::repo_visibility::RepoVisibility::Private.as_str()
            }))
    }
}
