use std::sync::Arc;

use rand::Rng;

use crate::repository::api_token::CreateSessionTokenParams;
use crate::repository::{
    Repositories, client_login_token::CreateClientLoginTokenParams,
    sso_login_token::CreateSsoLoginTokenParams,
};
use crate::service::auth::token::generate_api_token;
use base::error::AppError;
use infra::entity::sso_login_token;

/// Completion window: the browser must click "login to client" within 300s of
/// first opening `/client-sso/{token}/` (matches seahub's
/// `CLIENT_SSO_TOKEN_EXPIRATION`).
const SSO_COMPLETE_TIMEOUT_SECS: i64 = 300;

/// Service for SSO login flows, client login tokens, and device-wipe reporting.
pub struct SsoService {
    repos: Arc<Repositories>,
    api_token_ttl_days: u64,
}

impl SsoService {
    pub fn new(repos: Arc<Repositories>, api_token_ttl_days: u64) -> Self {
        Self {
            repos,
            api_token_ttl_days,
        }
    }

    /// Create a new SSO login token (POST /api2/client-login/).
    ///
    /// Generates a one-time token that a client can use to initiate the
    /// SSO browser-based authentication flow.
    pub async fn create_login_token(&self) -> Result<String, AppError> {
        let token = uuid::Uuid::new_v4().to_string();
        let now = chrono::Utc::now().timestamp();

        self.repos
            .sso_login_token
            .create_sso_token(CreateSsoLoginTokenParams {
                token: token.clone(),
                platform: None,
                device_id: None,
                device_name: None,
                client_version: None,
                status: "pending".to_string(),
                username: None,
                api_token: None,
                created_at: now,
                expires_at: Some(now + 3600),
            })
            .await?;
        Ok(token)
    }

    /// Create an SSO link token with optional device metadata
    /// (POST /api2/client-sso-link/).
    ///
    /// Returns the raw token; the handler builds the browser link from it.
    pub async fn create_sso_link(
        &self,
        platform: Option<String>,
        device_id: Option<String>,
        device_name: Option<String>,
        client_version: Option<String>,
    ) -> Result<String, AppError> {
        let token = uuid::Uuid::new_v4().to_string();
        let now = chrono::Utc::now().timestamp();

        self.repos
            .sso_login_token
            .create_sso_token(CreateSsoLoginTokenParams {
                token: token.clone(),
                platform,
                device_id,
                device_name,
                client_version,
                status: "waiting".to_string(),
                username: None,
                api_token: None,
                created_at: now,
                expires_at: Some(now + 3600),
            })
            .await?;

        Ok(token)
    }

    /// Record that the browser opened `/client-sso/{token}/`, starting the
    /// 300s completion window.
    pub async fn mark_accessed(&self, token: &str) -> Result<(), AppError> {
        let now = chrono::Utc::now().timestamp();
        self.repos.sso_login_token.mark_accessed(token, now).await
    }

    /// Open the browser link (`GET /client-sso/{token}/`).
    ///
    /// Marks the link as accessed on the first visit and returns `true`.
    /// Returns `false` if the link was already opened — the page should show
    /// the seahub-compatible "already visited" error.
    pub async fn open_sso_link(&self, token: &str) -> Result<bool, AppError> {
        let record = self
            .repos
            .sso_login_token
            .find_by_token(token)
            .await?
            .ok_or_else(|| AppError::NotFound("token not found".into()))?;

        if record.accessed_at.is_none() {
            self.mark_accessed(token).await?;
            Ok(true)
        } else {
            Ok(false)
        }
    }

    /// Validate that an SSO link can still be completed (used by the GET
    /// confirm page). Must have been opened in the browser and still be within
    /// the 300s completion window.
    pub async fn validate_sso_link_for_completion(&self, token: &str) -> Result<(), AppError> {
        let record = self
            .repos
            .sso_login_token
            .find_by_token(token)
            .await?
            .ok_or_else(|| AppError::NotFound("token not found".into()))?;
        check_link_valid(&record)
    }

    /// Poll the status of an SSO login token (GET /api2/client-sso-link/{token}/).
    ///
    /// Mirrors seahub's `ClientSSOLink.get`: returns the current status verbatim
    /// while not `success`, and only applies the `accessed_at`-based timeout to
    /// `success` rows. Unknown tokens surface as `AppError::NotFound` (404).
    pub async fn poll_sso_link(&self, token: &str) -> Result<PollResult, AppError> {
        let record = self
            .repos
            .sso_login_token
            .find_by_token(token)
            .await?
            .ok_or_else(|| AppError::NotFound("token not found".into()))?;

        if record.status != "success" {
            return Ok(PollResult::Status(record.status));
        }

        let now = chrono::Utc::now().timestamp();
        match record.accessed_at {
            None => Ok(PollResult::Status("error".to_string())),
            Some(ts) if now - ts >= SSO_COMPLETE_TIMEOUT_SECS => {
                Ok(PollResult::Status("error".to_string()))
            }
            Some(_) => match (record.username, record.api_token) {
                (Some(username), Some(api_token)) => Ok(PollResult::Success {
                    username,
                    api_token,
                }),
                _ => Ok(PollResult::Status("error".to_string())),
            },
        }
    }

    /// Complete the SSO flow (POST /client-sso/{token}/complete/).
    ///
    /// Verifies the link was opened in the browser and is still within the
    /// completion window, then mints an API token for the logged-in web user.
    /// When the desktop client supplied `shib_*` device params the token is
    /// device-bound (mirrors seahub's `get_token_v2`); otherwise it is a plain
    /// token (`get_token_v1`). Idempotent for already-`success` rows.
    pub async fn complete_sso_link(&self, token: &str, email: &str) -> Result<(), AppError> {
        let record = self
            .repos
            .sso_login_token
            .find_by_token(token)
            .await?
            .ok_or_else(|| AppError::NotFound("token not found".into()))?;

        check_link_valid(&record)?;

        // Already completed (seahub logs "not waiting, skip"). No new token.
        if record.status == "success" {
            return Ok(());
        }

        let now = chrono::Utc::now().timestamp();

        let user = self
            .repos
            .user
            .find_by_email(email)
            .await?
            .ok_or(AppError::Unauthorized)?;

        let api_token = generate_api_token();
        self.repos
            .api_token
            .create_session_token(CreateSessionTokenParams {
                user_id: user.id,
                token: api_token.clone(),
                created_at: now,
                expires_at: Some(now + (self.api_token_ttl_days as i64 * 86400)),
                device_id: record.device_id.clone(),
                platform: record.platform.clone(),
                device_name: record.device_name.clone(),
                client_version: record.client_version.clone(),
                is_pending: false,
            })
            .await?;

        self.repos
            .sso_login_token
            .complete(token, email, &api_token)
            .await?;

        Ok(())
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
            .create_client_login_token(CreateClientLoginTokenParams {
                token: token.clone(),
                username: email.to_string(),
                created_at: now,
            })
            .await?;

        Ok(token)
    }

    /// Report that a device was wiped (POST /api2/device-wiped/).
    ///
    /// Invalidates the API tokens belonging to `user_id` on `device_id`. The
    /// wipe report is scoped to the (user, device) that owns the reporting
    /// token, so it can never revoke another user's sessions.
    pub async fn device_wiped(&self, user_id: i32, device_id: &str) -> Result<(), AppError> {
        self.repos
            .api_token
            .delete_many_by_user_and_device(user_id, device_id)
            .await?;

        tracing::info!("device wiped: user_id={}, device_id={}", user_id, device_id);
        Ok(())
    }
}

/// Result of polling an SSO link.
pub enum PollResult {
    /// Return `{"status": <value>}` verbatim (e.g. "waiting", "error").
    Status(String),
    /// Return `{"status":"success","username":...,"apiToken":...}`.
    Success { username: String, api_token: String },
}

/// A link can be completed only if the browser opened it and the 300s window
/// (measured from `accessed_at`) has not elapsed. Mirrors seahub's
/// `client_sso_complete` validation.
fn check_link_valid(record: &sso_login_token::Model) -> Result<(), AppError> {
    let accessed_at = record.accessed_at.ok_or_else(|| {
        AppError::BadRequest(
            "Invalid link, please click the login button on the client again".to_string(),
        )
    })?;
    let now = chrono::Utc::now().timestamp();
    if now - accessed_at >= SSO_COMPLETE_TIMEOUT_SECS {
        return Err(AppError::BadRequest(
            "Login timeout, please click the login button on the client again".to_string(),
        ));
    }
    Ok(())
}
