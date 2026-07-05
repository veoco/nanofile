use sea_orm::DatabaseConnection;

use crate::error::AppError;
use crate::repository::Repositories;

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
pub struct AdminUserService<'a> {
    pub db: &'a DatabaseConnection,
    pub repos: &'a Repositories,
}

impl<'a> AdminUserService<'a> {
    pub fn new(db: &'a DatabaseConnection, repos: &'a Repositories) -> Self {
        Self { db, repos }
    }

    /// List all users with their storage usage.
    pub async fn list_users(&self) -> Result<Vec<UserAdminInfo>, AppError> {
        let users = self.repos.user.find_all().await?;
        let mut result = Vec::with_capacity(users.len());

        for u in users {
            let usage = self.compute_usage(u.id).await?;
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
        let owned_repos = self.repos.repo.find_by_owner_id(user_id).await?;
        Ok(owned_repos.iter().map(|r| r.size).sum())
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
