use axum::extract::FromRequestParts;
use axum::http::header;
use axum::http::request::Parts;
use axum::http::{HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use base64::Engine;
use std::sync::Arc;

use crate::AppState;
use crate::service::repo::webdav_key::hash_webdav_key;

/// Authenticated WebDAV request identity.
///
/// Username is a member's email; the password is one of that user's WebDAV
/// keys for the repo. Permission reflects the member's repo permission
/// ("rw" or "r").
#[derive(Debug, Clone)]
pub struct WebDavAuth {
    pub user_id: i32,
    pub email: String,
    pub repo_id: String,
    pub permission: String,
}

/// Rejection type for `WebDavAuth`. A 401 *must* carry a `WWW-Authenticate`
/// header or WebDAV clients will not prompt for credentials.
pub enum WebDavAuthError {
    Unauthorized,
    Forbidden,
    NotFound,
}

impl IntoResponse for WebDavAuthError {
    fn into_response(self) -> Response {
        match self {
            WebDavAuthError::Unauthorized => {
                let mut resp = StatusCode::UNAUTHORIZED.into_response();
                resp.headers_mut().insert(
                    header::WWW_AUTHENTICATE,
                    HeaderValue::from_static("Basic realm=\"nanofile webdav\""),
                );
                resp
            }
            WebDavAuthError::Forbidden => StatusCode::FORBIDDEN.into_response(),
            WebDavAuthError::NotFound => StatusCode::NOT_FOUND.into_response(),
        }
    }
}

impl FromRequestParts<Arc<AppState>> for WebDavAuth {
    type Rejection = WebDavAuthError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &Arc<AppState>,
    ) -> Result<Self, Self::Rejection> {
        // Global switch — WebDAV disabled entirely.
        if !state.config.server.webdav_enabled {
            return Err(WebDavAuthError::Forbidden);
        }

        // repo_id comes from the URI (`/dav/{repo_id}/...`) rather than the
        // axum Path extractor, so the same extractor works for the root and
        // `{*path}` route variants.
        let repo_id = {
            let mut segs = parts.uri.path().trim_start_matches('/').split('/');
            segs.next(); // "dav"
            segs.next()
                .filter(|s| !s.is_empty())
                .ok_or(WebDavAuthError::Unauthorized)?
                .to_string()
        };

        // Parse `Authorization: Basic base64(email:key)`.
        let auth_header = parts
            .headers
            .get(header::AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            .and_then(|s| s.strip_prefix("Basic "))
            .ok_or(WebDavAuthError::Unauthorized)?;
        let decoded = base64::engine::general_purpose::STANDARD
            .decode(auth_header)
            .map_err(|_| WebDavAuthError::Unauthorized)?;
        let creds = String::from_utf8(decoded).map_err(|_| WebDavAuthError::Unauthorized)?;
        let (email, key) = creds.split_once(':').ok_or(WebDavAuthError::Unauthorized)?;

        // User must exist and be active.
        let user = state
            .repos
            .user
            .find_by_email(email)
            .await
            .map_err(|_| WebDavAuthError::Unauthorized)?
            .ok_or(WebDavAuthError::Unauthorized)?;
        if !user.is_active {
            return Err(WebDavAuthError::Unauthorized);
        }

        // Repo must exist and be unencrypted (WebDAV has no way to supply the
        // library encryption password).
        let repo = state
            .repos
            .repo
            .find_by_id(&repo_id)
            .await
            .map_err(|_| WebDavAuthError::NotFound)?
            .ok_or(WebDavAuthError::NotFound)?;
        if repo.encrypted != 0 {
            return Err(WebDavAuthError::Forbidden);
        }

        // Determine permission from membership.
        let row = state
            .repos
            .member
            .find_repo_owner_and_permission(&repo_id, user.id)
            .await
            .map_err(|_| WebDavAuthError::Unauthorized)?;
        let permission = match row {
            Some((owner_id, _)) if owner_id == user.id => "rw",
            Some((_, Some(p))) if p == "rw" => "rw",
            Some((_, Some(_))) => "r",
            _ => return Err(WebDavAuthError::Unauthorized),
        };

        // The key must exist for this repo + user. Keys are stored as a
        // SHA-256 hash, so we hash the presented key and look it up.
        let key_hash = hash_webdav_key(key);
        let key_model = state
            .repos
            .webdav_key
            .find_by_repo_user_hash(&repo_id, user.id, &key_hash)
            .await
            .map_err(|_| WebDavAuthError::Unauthorized)?
            .ok_or(WebDavAuthError::Unauthorized)?;

        // Best-effort last_used_at update (fire-and-forget), throttled to once
        // per hour per key so high-frequency WebDAV traffic does not write on
        // every request.
        let now = chrono::Utc::now().timestamp();
        const THROTTLE_SECS: i64 = 60 * 60;
        let needs_update = match key_model.last_used_at {
            Some(ts) => now - ts >= THROTTLE_SECS,
            None => true,
        };
        if needs_update {
            let repo = state.repos.webdav_key.clone();
            let key_id = key_model.id;
            tokio::spawn(async move {
                let _ = repo.update_last_used_at(key_id, now).await;
            });
        }

        Ok(WebDavAuth {
            user_id: user.id,
            email: user.email,
            repo_id,
            permission: permission.to_string(),
        })
    }
}
