use std::sync::Arc;

use crate::repository::Repositories;
use base::error::AppError;

/// Information about a user for the admin user management page.
#[derive(serde::Serialize)]
pub struct UserAdminInfo {
    pub id: i32,
    pub email: String,
    pub is_active: bool,
    pub is_admin: bool,
    pub storage_quota: Option<i64>,
    pub usage: i64,
    pub created_at: i64,
    pub last_login_at: Option<i64>,
}

/// Service for admin user management operations.
pub struct AdminUserService {
    repos: Arc<Repositories>,
}

impl AdminUserService {
    pub fn new(repos: Arc<Repositories>) -> Self {
        Self { repos }
    }

    /// List all users with their storage usage.
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
            result.push(UserAdminInfo {
                id: u.id,
                email: u.email,
                is_active: u.is_active,
                is_admin: u.is_admin,
                storage_quota: u.storage_quota,
                usage,
                created_at: u.created_at,
                last_login_at: u.last_login_at,
            });
        }

        Ok(result)
    }

    /// Compute the total storage used by a user (sum of owned repo sizes).
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

    /// Update a user's admin status, active status, and storage quota.
    pub async fn update_user(
        &self,
        user_id: i32,
        is_admin: bool,
        is_active: bool,
        storage_quota: Option<i64>,
    ) -> Result<(), AppError> {
        self.repos.user.update_is_admin(user_id, is_admin).await?;
        self.repos.user.update_is_active(user_id, is_active).await?;
        self.repos
            .user
            .update_storage_quota(user_id, storage_quota)
            .await?;
        Ok(())
    }

    /// Delete a user by ID.
    pub async fn delete_user(&self, user_id: i32) -> Result<(), AppError> {
        self.repos.user.delete_user(user_id).await?;
        Ok(())
    }
}
