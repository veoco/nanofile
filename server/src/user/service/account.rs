use serde::Serialize;

use crate::error::AppError;
use crate::repository::Repositories;

#[derive(Serialize)]
pub struct AccountInfo {
    pub email: String,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub nickname: Option<String>,
    #[serde(rename = "id")]
    pub id: i32,
    /// Space used in bytes (sum of owned repo sizes).
    pub usage: i64,
    /// Storage quota in bytes. 0 means unlimited.
    pub total: i64,
}

pub struct AccountService<'a> {
    pub repos: &'a Repositories,
}

impl<'a> AccountService<'a> {
    pub fn new(repos: &'a Repositories) -> Self {
        Self { repos }
    }

    /// Fetch account info for a user, computing usage from owned repos.
    pub async fn get_account_info(
        &self,
        user_id: i32,
        max_storage_bytes: u64,
    ) -> Result<AccountInfo, AppError> {
        let user_record = self
            .repos
            .user
            .find_by_id(user_id)
            .await?
            .ok_or(AppError::Unauthorized)?;

        let usage = self.compute_usage(user_id).await?;
        let total = if max_storage_bytes > 0 {
            max_storage_bytes as i64
        } else {
            0
        };

        let nickname = user_record.nickname();

        Ok(AccountInfo {
            email: user_record.email.clone(),
            name: nickname.clone(),
            nickname: Some(nickname),
            id: user_record.id,
            usage,
            total,
        })
    }

    /// Update the user's display name / nickname.
    pub async fn update_account_info(
        &self,
        user_id: i32,
        name: String,
        max_storage_bytes: u64,
    ) -> Result<AccountInfo, AppError> {
        let display_name = if name.is_empty() {
            None
        } else {
            Some(name.trim().to_string())
        };

        self.repos
            .user
            .update_display_name(user_id, display_name)
            .await?;

        let user_record = self
            .repos
            .user
            .find_by_id(user_id)
            .await?
            .ok_or(AppError::Unauthorized)?;

        let usage = self.compute_usage(user_id).await?;
        let total = if max_storage_bytes > 0 {
            max_storage_bytes as i64
        } else {
            0
        };

        let nickname = user_record.nickname();

        Ok(AccountInfo {
            email: user_record.email.clone(),
            name: nickname.clone(),
            nickname: Some(nickname),
            id: user_record.id,
            usage,
            total,
        })
    }

    /// Register a new user. The password should already be hashed by the caller.
    pub async fn register_user(
        &self,
        email: String,
        password_hash: String,
    ) -> Result<(), AppError> {
        let existing = self.repos.user.find_by_email(&email).await?;

        if existing.is_some() {
            return Err(AppError::BadRequest("user already exists".into()));
        }

        self.repos.user.create(email, password_hash).await?;
        Ok(())
    }

    /// Sum of repo sizes owned by a user.
    async fn compute_usage(&self, user_id: i32) -> Result<i64, AppError> {
        let owned_repos = self.repos.repo.find_by_owner_id(user_id).await?;
        Ok(owned_repos.iter().map(|r| r.size).sum())
    }
}
