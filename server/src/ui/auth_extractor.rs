/// WebUser extractor — cookie-based auth for the Web UI.
///
/// Reads the `seahub-session` cookie, validates it against the `api_token`
/// table, and returns the authenticated user. Reuses the existing token
/// infrastructure — no new DB tables needed.
///
/// On failure: returns a 302 redirect to `/accounts/login/` instead of 401,
/// so browsers see the login page rather than a raw error.
use axum::{
    extract::FromRequestParts,
    http::{HeaderMap, header, request::Parts},
    response::{IntoResponse, Redirect, Response},
};
use axum_extra::headers::{Cookie, Header};
use std::sync::Arc;

use crate::AppState;

#[derive(Debug, Clone)]
pub struct WebUser {
    pub user_id: i32,
    pub email: String,
    /// The raw session token (for CSRF generation etc.).
    pub session_token: String,
    /// The `api_tokens` row this session is, so a page that lists sessions can
    /// mark the one the reader is using.
    pub session_id: i32,
    /// Whether the user has admin privileges.
    pub is_admin: bool,
    /// Preferred UI language from the user profile (None = browser default).
    pub language: Option<String>,
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
        let session_token =
            session_cookie(&parts.headers).ok_or(WebUserRejection::RedirectLogin)?;
        WebUser::from_session_token(state, &session_token)
            .await
            .ok_or(WebUserRejection::RedirectLogin)
    }
}

/// The `seahub-session` cookie from a request's headers, if it carries one.
///
/// The extractor above parses the headers it is handed; the error-page
/// middleware only has a clone of them (the request itself has already gone to
/// the handler), so both read the cookie through here.
pub fn session_cookie(headers: &HeaderMap) -> Option<String> {
    let mut values = headers.get_all(header::COOKIE).iter();
    let cookie = Cookie::decode(&mut values).ok()?;
    cookie.get("seahub-session").map(str::to_string)
}

impl WebUser {
    /// Resolve a session cookie value to the user it belongs to.
    ///
    /// `None` for every way a session can fail to identify a usable account:
    /// unknown token, expired token, a 2FA-pending token (which must not work
    /// as a full session), a deleted or deactivated user, or a failed lookup.
    pub async fn from_session_token(state: &Arc<AppState>, session_token: &str) -> Option<Self> {
        let token_record = state
            .repos
            .api_token
            .find_by_token(session_token)
            .await
            .ok()??;

        // Check expiration
        if let Some(expires_at) = token_record.expires_at
            && chrono::Utc::now().timestamp() > expires_at
        {
            return None;
        }

        // A 2FA pending token must not be usable as a full session.
        if token_record.is_pending {
            return None;
        }

        let user_record = state
            .repos
            .user
            .find_by_id(token_record.user_id)
            .await
            .ok()??;

        if !user_record.is_active {
            return None;
        }

        Some(WebUser {
            user_id: user_record.id,
            email: user_record.email,
            session_token: session_token.to_string(),
            session_id: token_record.id,
            is_admin: user_record.is_admin,
            language: user_record.language.clone(),
        })
    }
}
