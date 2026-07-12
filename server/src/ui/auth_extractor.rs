/// WebUser extractor — cookie-based auth for the Web UI.
///
/// Reads the `seahub-session` cookie, validates it against the `api_token`
/// table, and returns the authenticated user. Reuses the existing token
/// infrastructure — no new DB tables needed.
///
/// On failure: returns a 302 redirect to `/accounts/login/` instead of 401,
/// so browsers see the login page rather than a raw error.
use axum::{
    RequestPartsExt,
    extract::FromRequestParts,
    http::request::Parts,
    response::{IntoResponse, Redirect, Response},
};
use axum_extra::{TypedHeader, headers::Cookie};
use std::sync::Arc;

use crate::AppState;

#[derive(Debug, Clone)]
pub struct WebUser {
    pub user_id: i32,
    pub email: String,
    /// The raw session token (for CSRF generation etc.).
    pub session_token: String,
    /// Whether the user has admin privileges.
    pub is_admin: bool,
}

impl WebUser {
    /// Authenticate from a raw token string (used by test helpers).
    pub async fn from_token(
        repos: &crate::repository::Repositories,
        token_str: &str,
    ) -> Result<Self, ()> {
        let token_record = repos
            .api_token
            .find_by_token(token_str)
            .await
            .map_err(|_| ())?
            .ok_or(())?;

        // Check expiration
        if let Some(expires_at) = token_record.expires_at {
            let now = chrono::Utc::now().timestamp();
            if now > expires_at {
                return Err(());
            }
        }

        // Look up user
        let user_record = repos
            .user
            .find_by_id(token_record.user_id)
            .await
            .map_err(|_| ())?
            .ok_or(())?;

        if !user_record.is_active {
            return Err(());
        }

        Ok(WebUser {
            user_id: user_record.id,
            email: user_record.email,
            session_token: token_str.to_string(),
            is_admin: user_record.is_admin,
        })
    }
}

/// Rejection type for WebUser — redirects to the login page.
pub enum WebUserRejection {
    RedirectLogin,
}

impl IntoResponse for WebUserRejection {
    fn into_response(self) -> Response {
        Redirect::to("/accounts/login/").into_response()
    }
}

impl FromRequestParts<Arc<AppState>> for WebUser {
    type Rejection = WebUserRejection;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &Arc<AppState>,
    ) -> Result<Self, Self::Rejection> {
        // Extract the cookie header
        let cookie_header = parts
            .extract::<TypedHeader<Cookie>>()
            .await
            .map_err(|_| WebUserRejection::RedirectLogin)?;

        let session_token = cookie_header
            .get("seahub-session")
            .ok_or(WebUserRejection::RedirectLogin)?;

        // Look up the API token
        let token_record = state
            .repos
            .api_token
            .find_by_token(session_token)
            .await
            .map_err(|_| WebUserRejection::RedirectLogin)?
            .ok_or(WebUserRejection::RedirectLogin)?;

        // Check expiration
        if let Some(expires_at) = token_record.expires_at {
            let now = chrono::Utc::now().timestamp();
            if now > expires_at {
                return Err(WebUserRejection::RedirectLogin);
            }
        }

        // Look up user
        let user_record = state
            .repos
            .user
            .find_by_id(token_record.user_id)
            .await
            .map_err(|_| WebUserRejection::RedirectLogin)?
            .ok_or(WebUserRejection::RedirectLogin)?;

        if !user_record.is_active {
            return Err(WebUserRejection::RedirectLogin);
        }

        Ok(WebUser {
            user_id: user_record.id,
            email: user_record.email,
            session_token: session_token.to_string(),
            is_admin: user_record.is_admin,
        })
    }
}
