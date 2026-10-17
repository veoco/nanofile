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

    /// Unlink (revoke) a device by removing the credentials it owns.
    ///
    /// Scoped to the device: its sessions, its 2FA trust, and the sync tokens issued to it (a token's
    /// `peer_id` carries the client's `client_id`). Unlinking one device cannot touch another's tokens,
    /// and an unattributed (NULL `peer_id`) token stays listed in the credential inventory so it can be
    /// revoked one by one. A token left by an older build may still be shared by two devices: it is
    /// attributed to the first, so unlinking the *other* one does not revoke it.
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
