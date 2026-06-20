use std::sync::Arc;

use sea_orm::{
    DatabaseConnection, Set,
};

use rand::Rng;

use crate::entity::{client_login_token, sso_login_token};
use crate::error::AppError;
use crate::repository::Repositories;

/// Service for SSO login flows, client login tokens, and device-wipe reporting.
pub struct SsoService {
    #[allow(dead_code)]
    db: Arc<DatabaseConnection>,
    repos: Arc<Repositories>,
}

impl SsoService {
    pub fn new(db: Arc<DatabaseConnection>, repos: Arc<Repositories>) -> Self {
        Self { db, repos }
    }

    /// Create a new SSO login token (POST /api2/client-login/).
    ///
    /// Generates a one-time token that a client can use to initiate the
    /// SSO browser-based authentication flow.
    pub async fn create_login_token(&self) -> Result<String, AppError> {
        let token = uuid::Uuid::new_v4().to_string();
        let now = chrono::Utc::now().timestamp();

        let model = sso_login_token::ActiveModel {
            id: sea_orm::NotSet,
            token: Set(token.clone()),
            platform: Set(None),
            device_id: Set(None),
            device_name: Set(None),
            status: Set("pending".to_string()),
            username: Set(None),
            api_token: Set(None),
            created_at: Set(now),
            expires_at: Set(Some(now + 3600)),
        };
        self.repos.sso_login_token.insert(model).await?;
        Ok(token)
    }

    /// Create an SSO link token with optional device metadata
    /// (POST /api2/client-sso-link/).
    ///
    /// Returns the link path and the raw token.
    pub async fn create_sso_link(
        &self,
        platform: Option<String>,
        device_id: Option<String>,
        device_name: Option<String>,
    ) -> Result<SsoLinkResult, AppError> {
        let token = uuid::Uuid::new_v4().to_string();
        let now = chrono::Utc::now().timestamp();

        let model = sso_login_token::ActiveModel {
            id: sea_orm::NotSet,
            token: Set(token.clone()),
            platform: Set(platform),
            device_id: Set(device_id),
            device_name: Set(device_name),
            status: Set("pending".to_string()),
            username: Set(None),
            api_token: Set(None),
            created_at: Set(now),
            expires_at: Set(Some(now + 3600)),
        };
        self.repos.sso_login_token.insert(model).await?;

        Ok(SsoLinkResult {
            link: format!("/api2/client-sso-link/{token}/"),
            token,
        })
    }

    /// Poll the status of an SSO login token (GET /api2/client-sso-link/{token}/).
    ///
    /// Returns `None` if still pending, or `Some(api_token)` if the SSO flow completed.
    pub async fn poll_sso_link(&self, token: &str) -> Result<Option<String>, AppError> {
        let record = self
            .repos
            .sso_login_token
            .find_by_token(token)
            .await?
            .ok_or_else(|| AppError::NotFound("token not found".into()))?;

        if record.status == "done" {
            Ok(record.api_token)
        } else {
            Ok(None)
        }
    }

    /// Create a short-lived client login token for "view on website" flow
    /// (POST /api2/client-login/ in client_login.rs).
    ///
    /// Token is valid for 30 seconds (matching Seahub behavior).
    pub async fn create_client_login_token(&self, email: &str) -> Result<String, AppError> {
        let mut raw = [0u8; 16];
        rand::rng().fill_bytes(&mut raw);
        let token = hex::encode(raw);
        let now = chrono::Utc::now().timestamp();

        self.repos
            .client_login_token
            .insert(client_login_token::ActiveModel {
                token: Set(token.clone()),
                username: Set(email.to_string()),
                created_at: Set(now),
            })
            .await?;

        Ok(token)
    }

    /// Report that a device was wiped (POST /api2/device-wiped/).
    ///
    /// Invalidates all API tokens associated with the given device.
    pub async fn device_wiped(
        &self,
        user_id: i32,
        device_id: Option<&str>,
        platform: Option<&str>,
    ) -> Result<(), AppError> {
        if let Some(dev_id) = device_id {
            self.repos
                .api_token
                .delete_many_by_device(dev_id)
                .await?;

            tracing::info!(
                "device wiped: user_id={}, device_id={:?}, platform={:?}",
                user_id,
                device_id,
                platform,
            );
        }

        Ok(())
    }
}

/// Result of creating an SSO link.
pub struct SsoLinkResult {
    pub link: String,
    pub token: String,
}
