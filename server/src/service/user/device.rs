use std::sync::Arc;

use crate::repository::Repositories;
use base::error::AppError;

pub struct DeviceService {
    repos: Arc<Repositories>,
}

impl DeviceService {
    pub fn new(repos: Arc<Repositories>) -> Self {
        Self { repos }
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

        // Also revoke sync tokens not linked to any peer (created via
        // download-info / repo-tokens with peer_id NULL), so a device unlink
        // reliably revokes every sync token the user holds.
        let deleted_sync_unlinked = self.repos.sync_token.delete_by_user(user_id).await?;

        Ok(serde_json::json!({
            "success": true,
            "deleted_api_tokens": deleted_api,
            "deleted_s2fa_tokens": deleted_s2fa,
            "deleted_sync_tokens": deleted_sync + deleted_sync_unlinked,
        }))
    }
}
