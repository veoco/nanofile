use std::sync::Arc;

use rand::Rng;

use crate::domain::session_source::SessionSource;
use crate::repository::api_token::CreateSessionTokenParams;
use crate::repository::{
    Repositories, client_login_token::CreateClientLoginTokenParams,
    sso_login_token::CreateSsoLoginTokenParams,
};
use crate::service::auth::token::generate_api_token;
use base::error::AppError;
use infra::entity::sso_login_token;

/// Completion window: the browser must click "login to client" within 300s of
/// first opening `/client-sso/{token}/` (seahub's `CLIENT_SSO_TOKEN_EXPIRATION`).
const SSO_COMPLETE_TIMEOUT_SECS: i64 = 300;

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

    /// Create a new one-time SSO login token (POST /api2/client-login/).
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
    /// (POST /api2/client-sso-link/); returns the raw token.
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

    /// Record that the browser opened `/client-sso/{token}/`, starting the 300s
    /// completion window.
    pub async fn mark_accessed(&self, token: &str) -> Result<(), AppError> {
        let now = chrono::Utc::now().timestamp();
        self.repos.sso_login_token.mark_accessed(token, now).await
    }

    /// Open the browser link (`GET /client-sso/{token}/`).
    ///
    /// Marks the link as accessed on the first visit and returns `true`; `false`
    /// means it was already opened and the page shows the seahub "already visited"
    /// error.
    pub async fn open_sso_link(&self, token: &str) -> Result<bool, AppError> {
        let record = self
            .repos
            .sso_login_token
            .find_by_token(token)
            .await?
            .ok_or_else(|| AppError::NotFound("token not found".into()))?;

        // A link that aged out before its first visit is dead; delete it so the
        // hourly cleanup has less to do and the client sees a hard 4xx.
        if record
            .expires_at
            .is_some_and(|exp| chrono::Utc::now().timestamp() > exp)
        {
            let _ = self.repos.sso_login_token.delete_by_token(token).await;
            return Err(AppError::NotFound("token expired".into()));
        }

        if record.accessed_at.is_none() {
            self.mark_accessed(token).await?;
            Ok(true)
        } else {
            Ok(false)
        }
    }

    /// Validate that an SSO link can still be completed (used by the GET confirm
    /// page): it must have been opened in the browser and be within the 300s window.
    pub async fn validate_sso_link_for_completion(&self, token: &str) -> Result<(), AppError> {
        let record = self
            .repos
            .sso_login_token
            .find_by_token(token)
            .await?
            .ok_or_else(|| AppError::NotFound("token not found".into()))?;
        check_link_valid(&record)
    }

    /// A human-readable description of the client that created an SSO link, for
    /// the browser confirmation page.
    ///
    /// The desktop client reports `shib_platform`, `shib_device_name` and
    /// `shib_client_version`; the mobile clients send nothing, so the result is
    /// `None` and the page omits the line.
    pub async fn sso_link_requester(&self, token: &str) -> Option<String> {
        let record = self
            .repos
            .sso_login_token
            .find_by_token(token)
            .await
            .ok()??;

        let mut parts: Vec<String> = Vec::new();
        if let Some(name) = record.device_name.as_deref().filter(|s| !s.is_empty()) {
            parts.push(name.to_string());
        }
        if let Some(platform) = record.platform.as_deref().filter(|s| !s.is_empty()) {
            parts.push(format!("({platform})"));
        }
        if let Some(version) = record.client_version.as_deref().filter(|s| !s.is_empty()) {
            parts.push(format!("v{version}"));
        }
        if parts.is_empty() {
            return None;
        }
        Some(parts.join(" "))
    }

    /// Poll the status of an SSO login token (GET /api2/client-sso-link/{token}/).
    ///
    /// Mirrors seahub's `ClientSSOLink.get`: returns the current status verbatim
    /// while not `success`, and only applies the `accessed_at` timeout to `success`
    /// rows. Unknown tokens surface as `AppError::NotFound` (404).
    pub async fn poll_sso_link(&self, token: &str) -> Result<PollResult, AppError> {
        let record = self
            .repos
            .sso_login_token
            .find_by_token(token)
            .await?
            .ok_or_else(|| AppError::NotFound("token not found".into()))?;

        // The absolute lifetime applies to every status, and answering 404 (rather
        // than a non-"success" status) is what makes all three official clients stop
        // polling: Android and iOS keep re-polling any other non-empty status.
        let now = chrono::Utc::now().timestamp();
        if record.expires_at.is_some_and(|exp| now > exp) {
            let _ = self.repos.sso_login_token.delete_by_token(token).await;
            return Err(AppError::NotFound("token expired".into()));
        }

        if record.status != "success" {
            return Ok(PollResult::Status(record.status));
        }

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
    /// Verifies the link was opened in the browser and is still within the window,
    /// then mints an API token for the logged-in web user. With `shib_*` device
    /// params the token is device-bound (seahub's `get_token_v2`), otherwise plain
    /// (`get_token_v1`). Idempotent for already-`success` rows.
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
        // Any status other than "waiting" is not completable. Only "success" was
        // special-cased before, so a row in any other state could still be completed
        // (and mint a fresh account token).
        if record.status != "waiting" {
            return Err(AppError::BadRequest(
                "Invalid or expired link, please click the login button on the client again"
                    .to_string(),
            ));
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
                source: SessionSource::ClientSso,
                user_agent: None,
            })
            .await?;

        self.repos
            .sso_login_token
            .complete(token, email, &api_token)
            .await?;

        Ok(())
    }

    /// Create a short-lived client login token for the "view on website" flow
    /// (POST /api2/client-login/ in client_login.rs); valid for 30 seconds (Seahub).
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
    /// Drops every credential the wiped device holds: its API tokens, its repository
    /// sync tokens (used for `/seafhttp/`, up to `sync_token_ttl_days`) and its 2FA
    /// device trust; the same set `DeviceService::unlink_device` removes.
    /// The report is scoped to the (user, device) owning the reporting token, so it
    /// can never revoke another user's sessions.
    pub async fn device_wiped(&self, user_id: i32, device_id: &str) -> Result<(), AppError> {
        self.repos
            .api_token
            .delete_many_by_user_and_device(user_id, device_id)
            .await?;
        // A sync token's `peer_id` is the client's device id.
        self.repos
            .sync_token
            .delete_by_user_and_peer(user_id, device_id)
            .await?;
        self.repos
            .s2fa_token
            .delete_by_user_and_device(user_id, device_id)
            .await?;

        tracing::info!("device wiped: user_id={}, device_id={}", user_id, device_id);
        Ok(())
    }
}

pub enum PollResult {
    /// Return `{"status": <value>}` verbatim (e.g. "waiting", "error").
    Status(String),
    /// Return `{"status":"success","username":...,"apiToken":...}`.
    Success { username: String, api_token: String },
}

/// A link can be completed only if the browser opened it and the 300s window
/// (measured from `accessed_at`) has not elapsed; mirrors seahub's
/// `client_sso_complete`.
///
/// The absolute `expires_at` is checked too: without it the one-hour lifetime
/// would be a dead control and a captured link (referrer, shoulder-surfing, shared
/// terminal, proxy log) would be redeemable indefinitely.
fn check_link_valid(record: &sso_login_token::Model) -> Result<(), AppError> {
    let accessed_at = record.accessed_at.ok_or_else(|| {
        AppError::NotFound(
            "Invalid link, please click the login button on the client again".to_string(),
        )
    })?;
    let now = chrono::Utc::now().timestamp();

    // Unknown/aged-out links answer 404: every official client stops polling on a
    // 4xx, whereas a `{"status":"error"}` body makes Android and iOS keep polling.
    if record.expires_at.is_some_and(|exp| now > exp) {
        return Err(AppError::NotFound(
            "This link has expired, please click the login button on the client again".to_string(),
        ));
    }

    if now - accessed_at >= SSO_COMPLETE_TIMEOUT_SECS {
        return Err(AppError::BadRequest(
            "Login timeout, please click the login button on the client again".to_string(),
        ));
    }
    Ok(())
}

pub fn link_is_valid(record: &sso_login_token::Model) -> bool {
    check_link_valid(record).is_ok()
}
