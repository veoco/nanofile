use std::sync::Arc;

use serde::Serialize;

use crate::repository::Repositories;
use crate::service::user::primary_avatar_url;
use base::error::AppError;

#[derive(Serialize)]
pub struct AccountInfo {
    pub email: String,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub nickname: Option<String>,
    #[serde(rename = "id")]
    pub id: i32,
    pub usage: i64,
    /// Storage quota in bytes. -1 means unlimited.
    pub total: i64,
    /// Absolute URL to the user's avatar (seahub `avatar_url` compatibility).
    pub avatar_url: String,
    /// The user's contact email; nanofile has no separate contact-email profile, so it mirrors `email`.
    pub contact_email: String,
    /// Space usage as a percentage string (e.g. `"6.0382327%"`), matching seahub's field.
    pub space_usage: String,
}

pub struct AccountService {
    repos: Arc<Repositories>,
}

impl AccountService {
    pub fn new(repos: Arc<Repositories>) -> Self {
        Self { repos }
    }

    pub async fn get_account_info(
        &self,
        user_id: i32,
        default_quota: u64,
        site_url_origin: &str,
    ) -> Result<AccountInfo, AppError> {
        let user_record = self
            .repos
            .user
            .find_by_id(user_id)
            .await?
            .ok_or(AppError::Unauthorized)?;

        let usage = self.compute_usage(user_id).await?;
        let total = effective_quota_total(user_record.storage_quota, default_quota);

        let nickname = user_record.nickname();
        let email = user_record.email.clone();
        let avatar_url = format!("{}{}", site_url_origin, primary_avatar_url(&email, 80));
        let space_usage = format_space_usage(usage, total);

        Ok(AccountInfo {
            email: email.clone(),
            name: nickname.clone(),
            nickname: Some(nickname),
            id: user_record.id,
            usage,
            total,
            avatar_url,
            contact_email: email,
            space_usage,
        })
    }

    pub async fn update_account_info(
        &self,
        user_id: i32,
        name: String,
        default_quota: u64,
        site_url_origin: &str,
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
        let total = effective_quota_total(user_record.storage_quota, default_quota);

        let nickname = user_record.nickname();
        let email = user_record.email.clone();
        let avatar_url = format!("{}{}", site_url_origin, primary_avatar_url(&email, 80));
        let space_usage = format_space_usage(usage, total);

        Ok(AccountInfo {
            email: email.clone(),
            name: nickname.clone(),
            nickname: Some(nickname),
            id: user_record.id,
            usage,
            total,
            avatar_url,
            contact_email: email,
            space_usage,
        })
    }

    async fn compute_usage(&self, user_id: i32) -> Result<i64, AppError> {
        self.repos.compute_user_usage(user_id).await
    }
}

/// The effective storage quota in bytes: an explicit `0` means unlimited (`-1`),
/// a value is that many bytes, and `None` falls back to the server default
/// (`-1` when that is unlimited too). Shared by the account and admin responses.
pub fn effective_quota_total(storage_quota: Option<i64>, default_quota: u64) -> i64 {
    match storage_quota {
        Some(0) => -1,
        Some(n) => n,
        None => {
            if default_quota > 0 {
                default_quota as i64
            } else {
                -1
            }
        }
    }
}

/// Format space usage as a percentage string, matching seahub's `space_usage` (`"0%"` when unlimited).
fn format_space_usage(usage: i64, total: i64) -> String {
    if total > 0 {
        format!("{}%", usage as f64 / total as f64 * 100.0)
    } else {
        "0%".to_string()
    }
}
