use axum::extract::FromRequestParts;
use axum::http::header;
use axum::http::request::Parts;
use axum::http::{HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use base64::Engine;
use std::sync::Arc;

use crate::AppState;
use crate::domain::api_key::KeyAuthority;
use crate::domain::capability::{Capability, webdav_requires_write};

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
    /// Too many failed authentication attempts from this client address.
    TooManyRequests,
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
            WebDavAuthError::TooManyRequests => StatusCode::TOO_MANY_REQUESTS.into_response(),
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

        // Throttle failed attempts by client address before doing any database
        // work. Successful requests are never counted, so a working WebDAV
        // client cannot trip this even at high request rates.
        let peer = parts
            .extensions
            .get::<axum::extract::ConnectInfo<std::net::SocketAddr>>()
            .map(|ci| ci.0);
        let client_ip = match peer {
            Some(addr) => crate::middleware::effective_client_ip(
                &addr,
                &parts.headers,
                &state.config.server.trusted_proxies,
            ),
            // Without connect info every such request shares one bucket: that
            // throttles more aggressively, never less.
            None => "unknown".to_string(),
        };
        let rate_limit_key = format!("webdav:{client_ip}");
        if state.auth_limiters.webdav_auth.is_limited(&rate_limit_key) {
            return Err(WebDavAuthError::TooManyRequests);
        }

        let result = Self::authenticate(state, &repo_id, email, key, &parts.method).await;
        if matches!(result, Err(WebDavAuthError::Unauthorized)) {
            state
                .auth_limiters
                .webdav_auth
                .record_attempt(&rate_limit_key);
        } else if result.is_ok() {
            state.auth_limiters.webdav_auth.clear(&rate_limit_key);
        }
        result
    }
}

impl WebDavAuth {
    /// Resolve the user, repo and key. Split out so the caller can account for
    /// failures (and clear them on success) in one place.
    async fn authenticate(
        state: &Arc<AppState>,
        repo_id: &str,
        email: &str,
        key: &str,
        method: &axum::http::Method,
    ) -> Result<Self, WebDavAuthError> {
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
            .find_by_id(repo_id)
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
            .find_repo_owner_and_permission(repo_id, user.id)
            .await
            .map_err(|_| WebDavAuthError::Unauthorized)?;
        let permission = match row {
            Some((owner_id, _)) if owner_id == user.id => "rw",
            Some((_, Some(p))) if p == "rw" => "rw",
            Some((_, Some(_))) => "r",
            _ => return Err(WebDavAuthError::Unauthorized),
        };

        // The presented secret must be a unified API key that belongs to this
        // user, carries WebDAV access and covers this library. Keys are stored
        // as a SHA-256 hash, so only the hash is ever compared.
        let lookup = state
            .repos
            .api_key
            .find_by_presented(key)
            .await
            .map_err(|_| WebDavAuthError::Unauthorized)?
            .ok_or(WebDavAuthError::Unauthorized)?;
        if lookup.key.user_id != user.id {
            return Err(WebDavAuthError::Unauthorized);
        }
        let authority =
            KeyAuthority::from_lookup(&lookup).map_err(|_| WebDavAuthError::Unauthorized)?;
        if !authority.has(Capability::WebdavRead) {
            return Err(WebDavAuthError::Unauthorized);
        }
        let needs_write = webdav_requires_write(method.as_str());
        if needs_write && !authority.has(Capability::WebdavWrite) {
            return Err(WebDavAuthError::Forbidden);
        }
        if !authority.allows_repo(repo_id, needs_write) {
            return Err(WebDavAuthError::Forbidden);
        }

        // Effective permission is the stricter of the membership permission and
        // what the key allows — a read-only key stays read-only even for an rw
        // member, and a read-only member stays read-only even with an rw key.
        let key_allows_write =
            authority.has(Capability::WebdavWrite) && authority.allows_repo(repo_id, true);
        let permission = if permission == "rw" && key_allows_write {
            "rw"
        } else {
            "r"
        };
        if needs_write && permission != "rw" {
            return Err(WebDavAuthError::Forbidden);
        }

        // Best-effort last_used_at update (fire-and-forget), throttled to once
        // per hour per key so high-frequency WebDAV traffic does not write on
        // every request.
        let now = chrono::Utc::now().timestamp();
        const THROTTLE_SECS: i64 = 60 * 60;
        let needs_update = match lookup.key.last_used_at {
            Some(ts) => now - ts >= THROTTLE_SECS,
            None => true,
        };
        if needs_update {
            let keys = state.repos.api_key.clone();
            let key_id = lookup.key.id;
            tokio::spawn(async move {
                let _ = keys.touch_last_used(key_id, now).await;
            });
        }

        Ok(WebDavAuth {
            user_id: user.id,
            email: user.email,
            repo_id: repo_id.to_string(),
            permission: permission.to_string(),
        })
    }
}
