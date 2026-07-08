use axum::{
    extract::FromRequestParts,
    http::{StatusCode, request::Parts},
};

use crate::AppState;
use crate::repository::Repositories;

#[derive(Debug, Clone)]
pub struct AuthUser {
    pub user_id: i32,
    pub email: String,
}

#[derive(Debug, Clone)]
pub struct SyncAuth {
    pub user_id: i32,
    pub repo_id: String,
}

impl FromRequestParts<std::sync::Arc<AppState>> for SyncAuth {
    type Rejection = crate::error::AppError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &std::sync::Arc<AppState>,
    ) -> Result<Self, Self::Rejection> {
        let token = extract_sync_token(&parts.headers)?;
        let repos = &state.repos;

        // First try to authenticate via sync token (primary sync protocol path).
        // We look up sync_tokens directly here (rather than delegating to from_token)
        // so we can capture client_id from the request URI and store peer info.
        let sync_record = repos.sync_token.find_by_token(&token).await?;

        if let Some(record) = sync_record {
            // Check token expiry.
            if let Some(expires_at) = record.expires_at {
                let now = chrono::Utc::now().timestamp();
                if now > expires_at {
                    return Err(crate::error::AppError::Unauthorized);
                }
            }

            let user_id = record.user_id;
            let repo_id = record.repo_id.clone();

            // Capture client_id, client_name, client_ver from URL query params
            // and update the sync_token's peer info. This mirrors seafile-server's
            // RepoTokenPeerInfo table for device linking.
            if let Some(query) = parts.uri.query()
                && let Ok(params) =
                    serde_urlencoded::from_str::<std::collections::HashMap<String, String>>(query)
                && let Some(client_id) = params.get("client_id")
            {
                let now = chrono::Utc::now().timestamp();
                let peer_ip = parts
                    .headers
                    .get("x-forwarded-for")
                    .and_then(|v| v.to_str().ok())
                    .or_else(|| parts.headers.get("x-real-ip").and_then(|v| v.to_str().ok()))
                    .map(|s| s.to_string());
                let _ = repos
                    .sync_token
                    .update_peer_info(
                        record,
                        Some(client_id.clone()),
                        params.get("client_name").cloned(),
                        peer_ip,
                        params.get("client_ver").cloned(),
                        Some(now),
                    )
                    .await;
            }

            return Ok(SyncAuth { user_id, repo_id });
        }

        // Fall back to API token (for requests using Bearer/Token auth).
        SyncAuth::from_token(repos, &token)
            .await
            .map_err(|_| crate::error::AppError::Unauthorized)
    }
}

impl FromRequestParts<std::sync::Arc<AppState>> for AuthUser {
    type Rejection = StatusCode;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &std::sync::Arc<AppState>,
    ) -> Result<Self, Self::Rejection> {
        let repos = &state.repos;

        // ── Path 1: Authorization header (external API clients) ──────────────
        let token_str = extract_auth_header_token(&parts.headers);

        // ── Path 2: Session cookie + CSRF header (browser UI requests) ──────
        // CSRF check is skipped for safe methods (GET, HEAD) because browsers
        // cannot attach custom headers for <img>, <link>, <script> etc.
        let token_str = match token_str {
            Some(t) => t,
            None => match try_extract_cookie_session(&parts.headers, state, &parts.method) {
                Some(t) => t,
                None => return Err(StatusCode::UNAUTHORIZED),
            },
        };

        // Query both token tables concurrently.
        let api_fut = repos.api_token.find_by_token(&token_str);
        let sync_fut = repos.sync_token.find_by_token(&token_str);

        let (api_result, sync_result) = tokio::join!(api_fut, sync_fut);

        let api_record = api_result.map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
        let sync_record = sync_result.map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

        let user_id = if let Some(token_record) = api_record {
            // Check API token expiration.
            if let Some(expires_at) = token_record.expires_at {
                let now = chrono::Utc::now().timestamp();
                if now > expires_at {
                    return Err(StatusCode::UNAUTHORIZED);
                }
            }
            token_record.user_id
        } else if let Some(sync_rec) = sync_record {
            if let Some(expires_at) = sync_rec.expires_at {
                let now = chrono::Utc::now().timestamp();
                if now > expires_at {
                    return Err(StatusCode::UNAUTHORIZED);
                }
            }
            sync_rec.user_id
        } else {
            return Err(StatusCode::UNAUTHORIZED);
        };

        let user_record = repos
            .user
            .find_by_id(user_id)
            .await
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
            .ok_or(StatusCode::UNAUTHORIZED)?;

        if !user_record.is_active {
            return Err(StatusCode::FORBIDDEN);
        }

        Ok(AuthUser {
            user_id: user_record.id,
            email: user_record.email,
        })
    }
}

/// Extract a token from `Authorization: Bearer <token>` or `Authorization: Token <token>`.
fn extract_auth_header_token(headers: &axum::http::HeaderMap) -> Option<String> {
    // Prefer the strongly-typed Bearer extraction.
    // We cannot use TypedHeader here because we only have headers, not Parts,
    // so we parse the header manually as a fallback.
    if let Some(auth) = headers.get("authorization").and_then(|v| v.to_str().ok()) {
        if let Some(token) = auth.strip_prefix("Token ") {
            return Some(token.to_string());
        } else if let Some(token) = auth.strip_prefix("Bearer ") {
            return Some(token.to_string());
        }
    }
    None
}

/// Try to authenticate via `seahub-session` cookie + `X-CSRFToken` header.
///
/// This is the **browser UI** path: the browser automatically sends the HttpOnly
/// session cookie, and JavaScript reads `sfcsrftoken` from its non-HttpOnly cookie
/// and echoes it back as `X-CSRFToken`.
fn try_extract_cookie_session(
    headers: &axum::http::HeaderMap,
    state: &std::sync::Arc<AppState>,
    method: &axum::http::Method,
) -> Option<String> {
    let cookie_str = headers.get("cookie").and_then(|v| v.to_str().ok())?;

    let session_token = cookie_str
        .split(';')
        .map(|s| s.trim())
        .find(|s| s.starts_with("seahub-session="))
        .and_then(|s| s.strip_prefix("seahub-session="))?;

    // ── CSRF check ──
    // Only required for state-changing methods (POST, PUT, PATCH, DELETE, etc.).
    // Safe methods (GET, HEAD, OPTIONS) are idempotent and cannot cause side
    // effects — browsers routinely issue them from <img>, <link>, <script> tags
    // that cannot attach custom headers.
    if *method != axum::http::Method::GET
        && *method != axum::http::Method::HEAD
        && !crate::auth::csrf::validate_csrf_header(headers, &state.csrf_secret, session_token)
    {
        return None;
    }

    Some(session_token.to_string())
}

impl SyncAuth {
    /// Authenticate via sync token or API token.
    pub async fn from_token(repos: &Repositories, token_str: &str) -> Result<Self, StatusCode> {
        // Query both token tables concurrently.
        let sync_fut = repos.sync_token.find_by_token(token_str);
        let api_fut = repos.api_token.find_by_token(token_str);

        let (sync_result, api_result) = tokio::join!(sync_fut, api_fut);

        // Check sync token first (has repo_id — preferred).
        if let Ok(Some(record)) = sync_result
            && let Ok(_) = &api_result
        {
            if let Some(expires_at) = record.expires_at {
                let now = chrono::Utc::now().timestamp();
                if now > expires_at {
                    return Err(StatusCode::UNAUTHORIZED);
                }
            }

            return Ok(SyncAuth {
                user_id: record.user_id,
                repo_id: record.repo_id,
            });
        }

        // Fall back to API token — check expiration like AuthUser does.
        if let Ok(Some(record)) = api_result {
            if let Some(expires_at) = record.expires_at {
                let now = chrono::Utc::now().timestamp();
                if now > expires_at {
                    return Err(StatusCode::UNAUTHORIZED);
                }
            }
            return Ok(SyncAuth {
                user_id: record.user_id,
                repo_id: String::new(),
            });
        }

        Err(StatusCode::UNAUTHORIZED)
    }
}

/// Extract a sync/API token from HTTP headers. Used by /seafhttp/ endpoints.
/// Checks the Seafile-Repo-Token header first, then the Authorization header.
pub fn extract_sync_token(
    headers: &axum::http::HeaderMap,
) -> Result<String, crate::error::AppError> {
    use crate::error::AppError;

    if let Some(token) = headers
        .get("Seafile-Repo-Token")
        .and_then(|v| v.to_str().ok())
    {
        return Ok(token.to_string());
    }

    if let Some(auth) = headers.get("Authorization").and_then(|v| v.to_str().ok())
        && let Some(token) = auth.strip_prefix("Token ")
    {
        return Ok(token.to_string());
    }

    Err(AppError::Unauthorized)
}
