use sea_orm::DatabaseConnection;

use crate::error::AppError;
use crate::repository::Repositories;

pub struct DeviceService<'a> {
    pub db: &'a DatabaseConnection,
    pub repos: &'a Repositories,
}

impl<'a> DeviceService<'a> {
    pub fn new(db: &'a DatabaseConnection, repos: &'a Repositories) -> Self {
        Self { db, repos }
    }

    /// List all devices connected to a user's account.
    pub async fn list_devices(&self, user_id: i32) -> Result<Vec<serde_json::Value>, AppError> {
        let tokens = self
            .repos
            .api_token
            .find_by_user_id_with_platform(user_id)
            .await?;

        let mut seen = std::collections::HashSet::new();
        let mut devices = Vec::new();

        for token in tokens {
            let key = (token.platform.clone(), token.device_id.clone());
            if seen.insert(key) {
                let is_desktop = matches!(
                    token.platform.as_deref(),
                    Some("windows") | Some("linux") | Some("mac")
                );
                devices.push(serde_json::json!({
                    "key": token.token,
                    "platform": token.platform,
                    "device_id": token.device_id,
                    "device_name": token.device_name,
                    "client_version": token.client_version,
                    "last_accessed": token.created_at,
                    "is_desktop_client": is_desktop,
                }));
            }
        }

        Ok(devices)
    }

    /// Unlink (revoke) a device by removing all its tokens.
    pub async fn unlink_device(
        &self,
        user_id: i32,
        platform: &str,
        device_id: &str,
    ) -> Result<serde_json::Value, AppError> {
        let deleted_api = self
            .repos
            .api_token
            .delete_many_by_user_platform_device(user_id, platform, device_id)
            .await?;

        let deleted_s2fa = self
            .repos
            .s2fa_token
            .delete_by_user_and_device(user_id, device_id)
            .await?;

        let deleted_sync = self
            .repos
            .sync_token
            .delete_by_user_and_peer(user_id, device_id)
            .await?;

        Ok(serde_json::json!({
            "success": true,
            "deleted_api_tokens": deleted_api,
            "deleted_s2fa_tokens": deleted_s2fa,
            "deleted_sync_tokens": deleted_sync,
        }))
    }
}
