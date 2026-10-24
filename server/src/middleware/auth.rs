use axum::{
    Json,
    extract::FromRequestParts,
    http::{HeaderMap, StatusCode, request::Parts},
    response::{IntoResponse, Response},
};
use axum_extra::headers::{Cookie, Header};

use crate::AppState;
use crate::domain::api_key::KeyAuthority;
use crate::domain::capability::{RouteAccess, required_access};
use crate::domain::credential::Credential;
use crate::domain::device::DeviceIdentity;
use crate::domain::permission::RepoScope;
use crate::domain::repo_path;
use crate::repository::Repositories;
use base::error::AppError;

#[derive(Debug, Clone)]
pub struct AuthUser {
    pub user_id: i32,
    pub email: String,
    /// What authenticated the request; never [`Credential::SyncToken`], which
    /// only [`SyncAuth`] accepts.
    pub credential: Credential,
    /// The device the credential carries: a client API token records the
    /// `device_id` its login reported, while sessions have none. Sync-token
    /// issuance attaches the token to it instead of waiting for the first sync.
    pub device: Option<DeviceIdentity>,
}

impl AuthUser {
    /// The libraries an account-wide query may return.
    pub fn repo_scope(&self) -> RepoScope {
        self.credential.repo_scope()
    }

    /// Enforce a key's library scope for a repo id the path guard cannot see,
    /// as some endpoints carry the library in a query parameter or JSON body.
    pub fn ensure_repo_allowed(&self, repo_id: &str, need_write: bool) -> Result<(), AppError> {
        if self.credential.allows_repo(repo_id, need_write) {
            Ok(())
        } else {
            Err(AppError::Forbidden)
        }
    }
}

#[derive(Debug, Clone)]
pub struct SyncAuth {
    pub user_id: i32,
    pub repo_id: String,
    /// What authenticated the request: a repository sync token, an account
    /// token, or a unified API key.
    pub credential: Credential,
    /// The device behind the request when it is known: the presented sync
    /// token's peer, the account token's `device_id`, or the URL's `client_id`.
    pub device: Option<DeviceIdentity>,
}

impl SyncAuth {
    pub fn repo_scope(&self) -> RepoScope {
        self.credential.repo_scope()
    }
}

impl FromRequestParts<std::sync::Arc<AppState>> for SyncAuth {
    type Rejection = base::error::AppError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &std::sync::Arc<AppState>,
    ) -> Result<Self, Self::Rejection> {
        let token = extract_sync_token(&parts.headers)?;
        let repos = &state.repos;

        // Repo id embedded in the URL path, if any (`/seafhttp/repo/{repo_id}/…`).
        // Endpoints without a repo segment (accessible-repos, head-commits-multi)
        // skip the binding check. A present but non-UUID segment is rejected
        // outright (see `extract_url_repo_id`): axum's `Path` hands the handler
        // the *decoded* value, so treating it as "no repo" would let a
        // percent-encoded id (`%63cab3e0-…`) bypass the binding check.
        let url_repo_id =
            extract_url_repo_id(parts.uri.path()).map_err(|_| base::error::AppError::Forbidden)?;

        // Sync token first, looked up here rather than via `from_token` so the
        // request's client hints can be recorded against it.
        let sync_record = repos.sync_token.find_by_token(&token).await?;

        if let Some(record) = sync_record {
            // seaf-daemon only understands 403 (permission) and 400 (malformed),
            // so an invalid or cross-repo token is 403 — matching seafile's
            // fileserver (`server/http-server.c`, `fileserver/sync_api.go`).
            if is_token_expired(record.expires_at) {
                return Err(base::error::AppError::Forbidden);
            }

            // A sync token is bound to exactly one repo: it must match the URL's.
            if let Some(url_repo) = &url_repo_id
                && record.repo_id != *url_repo
            {
                return Err(base::error::AppError::Forbidden);
            }

            let user_id = record.user_id;
            let repo_id = record.repo_id.clone();

            // A deactivated or unverified account must not keep syncing.
            if !repos
                .user
                .find_by_id(user_id)
                .await?
                .is_some_and(|u| u.is_active && u.email_verified)
            {
                return Err(base::error::AppError::Forbidden);
            }

            // The sync protocol names the device in the URL query (`seaf-daemon`
            // adds it to `permission-check`; see `fileserver/sync_api.go`).
            let hint = client_hint(parts);

            // Peer info is insert-once, so one device can never take another's
            // token over (upstream's `RepoTokenPeerInfo` behaves the same).
            if let Some(device) = &hint
                && record.peer_id.as_deref().is_none_or(str::is_empty)
            {
                let _ = repos
                    .sync_token
                    .attach_peer_if_unset(record.id, &device.peer())
                    .await;
            }

            // Volatile columns follow each request, throttled per token.
            if let Some(device) = &hint
                && repos.peer_info_writes.allows_now(record.id)
            {
                let now = chrono::Utc::now().timestamp();
                let peer_ip = parts
                    .extensions
                    .get::<axum::extract::ConnectInfo<std::net::SocketAddr>>()
                    .map(|ci| {
                        crate::middleware::effective_client_ip(
                            &ci.0,
                            &parts.headers,
                            &state.config().server.trusted_proxies,
                        )
                    });
                let _ = repos
                    .sync_token
                    .touch_peer(record.id, peer_ip, device.client_version.clone(), now)
                    .await;
            }

            let device = device_from_token(&record).or(hint);
            return Ok(SyncAuth {
                user_id,
                repo_id,
                credential: Credential::SyncToken,
                device,
            });
        }

        // Fall back to a unified API key or a session API token; neither has a
        // device, so the URL's `client_id` is the only identity available.
        let mut auth = match SyncAuth::from_token(
            repos,
            &token,
            url_repo_id.as_deref(),
            &parts.method,
            parts.uri.path(),
        )
        .await
        {
            Ok(auth) => auth,
            Err(_) => {
                // seafile's file server checks that the library still exists
                // *before* validating credentials and answers 444
                // (SEAF_HTTP_RES_REPO_DELETED) when it is gone; the desktop client
                // turns exactly 444 into SYNC_ERROR_ID_SERVER_REPO_DELETED and
                // offers to remove its local copy (`sync-mgr.c:
                // on_repo_deleted_on_server`). A repo token dies with its library,
                // so without this a deleted library only ever looked like 403.
                if let Some(url_repo) = &url_repo_id
                    && repos.repo.find_by_id(url_repo).await?.is_none()
                {
                    return Err(base::error::AppError::RepoDeleted);
                }
                return Err(base::error::AppError::Forbidden);
            }
        };
        if auth.device.is_none() {
            auth.device = client_hint(parts);
        }
        Ok(auth)
    }
}

/// The only `/seafhttp/` endpoints allowed to skip the token↔repo binding check.
const NON_REPO_SYNC_ENDPOINTS: [&str; 3] =
    ["accessible-repos", "head-commits-multi", "protocol-version"];

/// Extract the repo id from a `/seafhttp/repo/{repo_id}/…` URL path.
///
/// `nest` strips the matched prefix and `SyncAuth` runs at several depths, so
/// both the full form and the stripped form (`/{id}/…`) are accepted.
///
/// The path is percent-decoded **before** the UUID check because axum's `Path`
/// extractor also decodes it. Without this, `/seafhttp/repo/%63cab3e0-…/commit`
/// would look like a non-UUID segment (→ `None` → binding check skipped) while
/// the handler saw the victim's real repo id — letting any sync token read any
/// repo. The check is fail-closed: only the explicit
/// [`NON_REPO_SYNC_ENDPOINTS`] allow-list may return `Ok(None)`.
fn extract_url_repo_id(path: &str) -> Result<Option<String>, ()> {
    let decoded = percent_encoding::percent_decode_str(path)
        .decode_utf8()
        .map_err(|_| ())?;
    let trimmed = decoded.trim_start_matches('/');
    let candidates = crate::domain::repo_path::candidates(trimmed);

    if let Some(repo_id) = candidates
        .iter()
        .find(|c| crate::domain::repo_path::is_uuid(c))
    {
        return Ok(Some((*repo_id).to_string()));
    }

    // No UUID anywhere: this is only legitimate for a known non-repo endpoint.
    if NON_REPO_SYNC_ENDPOINTS.contains(&candidates[0]) {
        return Ok(None);
    }

    Err(())
}

/// Rejection for [`AuthUser`]: the status, plus the JSON error document every other API surface
/// returns.
///
/// This used to be a bare [`StatusCode`], so an unauthenticated `/api2/…` request got a 401 with an
/// empty body while every other error on the same endpoint carried `{"error_msg": …}`. Clients that
/// surface the server's message showed nothing.
pub struct AuthRejection(StatusCode);

impl From<StatusCode> for AuthRejection {
    fn from(status: StatusCode) -> Self {
        Self(status)
    }
}

impl AuthRejection {
    /// The status this rejection answers with, for extractors that wrap [`AuthUser`] and carry
    /// their own error shape.
    pub fn status(&self) -> StatusCode {
        self.0
    }
}

impl IntoResponse for AuthRejection {
    fn into_response(self) -> Response {
        let status = self.0;
        // A 5xx must not describe the failure; the cause is logged where it happens.
        let message = match status {
            StatusCode::UNAUTHORIZED => {
                "Authentication credentials were not provided or are invalid."
            }
            StatusCode::FORBIDDEN => "You do not have permission to perform this action.",
            _ => "Internal server error",
        };
        (status, Json(serde_json::json!({ "error_msg": message }))).into_response()
    }
}

impl FromRequestParts<std::sync::Arc<AppState>> for AuthUser {
    type Rejection = AuthRejection;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &std::sync::Arc<AppState>,
    ) -> Result<Self, Self::Rejection> {
        let repos = &state.repos;

        // Authorization header (external API clients).
        let token_str = extract_auth_header_token(&parts.headers);

        // CSRF check is skipped for safe methods (GET, HEAD): browsers cannot
        // attach custom headers for <img>, <link>, <script> etc.
        let token_str = match token_str {
            Some(t) => t,
            None => match try_extract_cookie_session(&parts.headers, state, &parts.method) {
                Some(t) => t,
                None => return Err(StatusCode::UNAUTHORIZED.into()),
            },
        };

        // Only account credentials authenticate `/api2/*` and the Web UI: unified
        // API keys and session API tokens. Repository-scoped sync tokens are
        // accepted only by [`SyncAuth`], mirroring seahub's `TokenAuthentication`;
        // treating one as an account session would let a leaked library
        // credential read the whole account. A key and a session token cannot share
        // a value (both 160-bit random), so the two lookups run together.
        let (key_lookup, api_record) = tokio::join!(
            repos.api_key.find_by_presented(&token_str),
            repos.api_token.find_by_token(&token_str),
        );
        let key_lookup = key_lookup.map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
        let api_record = api_record.map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

        let (user_id, credential, device) = if let Some(lookup) = key_lookup {
            if is_token_expired(lookup.key.expires_at) {
                return Err(StatusCode::UNAUTHORIZED.into());
            }
            let authority = KeyAuthority::from_lookup(&lookup).map_err(|error| {
                // An unreadable grant means this build cannot honour what the
                // row promises; refusing the credential is the safe direction.
                tracing::warn!(key_id = lookup.key.id, %error, "rejecting API key with an unreadable capability set");
                StatusCode::UNAUTHORIZED
            })?;
            // Note the key as used before the route guard decides.
            note_key_usage(repos, lookup.key.id).await;
            let credential = Credential::Key(authority);
            enforce_route_access(&credential, parts)?;
            // A unified API key belongs to no device.
            (lookup.key.user_id, credential, None)
        } else if let Some(token_record) = api_record {
            if is_token_expired(token_record.expires_at) {
                return Err(StatusCode::UNAUTHORIZED.into());
            }
            // A 2FA pending token must not be usable as a full session.
            if token_record.is_pending {
                return Err(StatusCode::UNAUTHORIZED.into());
            }
            let credential = Credential::Session;
            enforce_route_access(&credential, parts)?;
            let device = device_from_api_token(&token_record);
            (token_record.user_id, credential, device)
        } else {
            return Err(StatusCode::UNAUTHORIZED.into());
        };

        let user_record = repos
            .user
            .find_by_id(user_id)
            .await
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
            .ok_or(StatusCode::UNAUTHORIZED)?;

        if !user_record.is_active || !user_record.email_verified {
            return Err(StatusCode::FORBIDDEN.into());
        }

        Ok(AuthUser {
            user_id: user_record.id,
            email: user_record.email,
            credential,
            device,
        })
    }
}

/// The device an account token was issued to, when its login reported one.
fn device_from_api_token(record: &infra::entity::api_token::Model) -> Option<DeviceIdentity> {
    DeviceIdentity::new(
        record.platform.clone(),
        record.device_id.clone(),
        record.device_name.clone(),
        record.client_version.clone(),
    )
}

/// The device a sync token is already attributed to.
fn device_from_token(record: &infra::entity::sync_token::Model) -> Option<DeviceIdentity> {
    DeviceIdentity::new(
        None,
        record.peer_id.clone(),
        record.peer_name.clone(),
        record.client_version.clone(),
    )
}

/// The device the sync protocol named in the request URL.
///
/// `seaf-daemon` appends `client_id`/`client_name`/`client_ver` (upstream's
/// fileserver reads them in `permissionCheckCB`), so they identify the device
/// even when the credential carries none.
fn client_hint(parts: &Parts) -> Option<DeviceIdentity> {
    let params: std::collections::HashMap<String, String> =
        serde_urlencoded::from_str(parts.uri.query().unwrap_or_default()).ok()?;
    DeviceIdentity::new(
        None,
        params.get("client_id").cloned(),
        params.get("client_name").cloned(),
        params.get("client_ver").cloned(),
    )
}

/// Apply a credential's capability and library limits to a request.
///
/// Every credential goes through here, so the route table is the single place
/// that decides which caller may reach which route; a session satisfies every
/// capability but is still classified, so an unclassified route fails closed for
/// everyone and a new endpoint stays unreachable until it is classified.
///
/// Classification uses the router's matched path *template*, not the raw URI
/// (`nest` leaves the raw path as `/{repo_id}/dir/` where the template is
/// `/api2/repos/{repo_id}/dir/`); library binding uses the raw path, where the
/// concrete id lives.
fn enforce_route_access(credential: &Credential, parts: &Parts) -> Result<(), StatusCode> {
    let matched = parts
        .extensions
        .get::<axum::extract::MatchedPath>()
        .map(axum::extract::MatchedPath::as_str);
    let uri_path = parts.uri.path();
    route_decision(
        credential,
        &parts.method,
        matched.unwrap_or(uri_path),
        uri_path,
    )
}

/// The policy half of [`enforce_route_access`], split out so the branch that
/// denies an unclassified route is testable without a real router.
fn route_decision(
    credential: &Credential,
    method: &axum::http::Method,
    route: &str,
    uri_path: &str,
) -> Result<(), StatusCode> {
    let Some(access) = required_access(method, route) else {
        // Fail closed and leave a trace: a gap in the table is a bug, so it must
        // be observable. `record` answers true only for a route's first hit.
        if super::route_audit::record(method.as_str(), uri_path) {
            tracing::warn!(
                %method,
                path = uri_path,
                credential = credential.kind_id(),
                "credential used on a route with no classification"
            );
        }
        return Err(StatusCode::FORBIDDEN);
    };
    if !credential.allows_route(access) {
        return Err(StatusCode::FORBIDDEN);
    }
    // A key carries a per-library ceiling on top of the route requirement; every
    // other credential is the account itself, so `allows_repo` accepts.
    if let RouteAccess::Capability(capability) = access
        && let Some(repo_id) = repo_path::find_repo_id(uri_path)
        && !credential.allows_repo(&repo_id, capability.is_write())
    {
        return Err(StatusCode::FORBIDDEN);
    }
    Ok(())
}

/// Extract a token from `Authorization: Bearer <token>` or `Authorization: Token <token>`.
fn extract_auth_header_token(headers: &axum::http::HeaderMap) -> Option<String> {
    // TypedHeader needs Parts, which we lack here, so parse the header by hand.
    if let Some(auth) = headers.get("authorization").and_then(|v| v.to_str().ok()) {
        if let Some(token) = auth.strip_prefix("Token ") {
            return Some(token.to_string());
        } else if let Some(token) = auth.strip_prefix("Bearer ") {
            return Some(token.to_string());
        }
    }
    None
}

/// The value of one cookie from a request's `Cookie` headers.
///
/// One parser for every caller: the Web UI extractor and this module each scanned the header
/// themselves before, so a header the HTTP library can decode (quoted values, several `Cookie`
/// headers) authenticated on one surface and not the other.
///
/// `Cookie::decode` is the primary path because it knows those rules, but it rejects the whole
/// header on a segment it cannot parse — and a browser will happily send one. The plain scan stays
/// as a fallback so a malformed neighbour does not turn a working session into a 401.
pub fn cookie_value(headers: &HeaderMap, name: &str) -> Option<String> {
    if let Ok(cookie) = Cookie::decode(&mut headers.get_all("cookie").iter())
        && let Some(value) = cookie.get(name)
    {
        return Some(value.to_string());
    }
    headers
        .get_all("cookie")
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|header| header.split(';'))
        .map(str::trim)
        .find_map(|pair| {
            pair.strip_prefix(name)
                .and_then(|rest| rest.strip_prefix('='))
                .map(str::to_string)
        })
}

/// The `seahub-session` cookie's value, if the request carries one.
pub fn session_cookie_value(headers: &HeaderMap) -> Option<String> {
    cookie_value(headers, "seahub-session")
}

/// Authenticate via the `seahub-session` cookie plus an `X-CSRFToken` header:
/// JavaScript reads `sfcsrftoken` from its non-HttpOnly cookie and echoes it,
/// while the browser sends the HttpOnly session cookie automatically.
fn try_extract_cookie_session(
    headers: &HeaderMap,
    state: &std::sync::Arc<AppState>,
    method: &axum::http::Method,
) -> Option<String> {
    let session_token = session_cookie_value(headers)?;

    // CSRF applies to state-changing methods only. Safe methods are idempotent
    // and come from tags that cannot attach custom headers.
    if *method != axum::http::Method::GET
        && *method != axum::http::Method::HEAD
        && !crate::service::auth::csrf::validate_csrf_header(
            headers,
            &state.csrf_secret,
            &session_token,
        )
    {
        return None;
    }

    Some(session_token)
}

impl SyncAuth {
    /// Authenticate a sync request; `url_repo_id` is the repo in the URL path,
    /// which a sync token must match and whose membership an API token's caller
    /// must hold.
    pub async fn from_token(
        repos: &Repositories,
        token_str: &str,
        url_repo_id: Option<&str>,
        method: &axum::http::Method,
        path: &str,
    ) -> Result<Self, StatusCode> {
        let sync_fut = repos.sync_token.find_by_token(token_str);
        let api_fut = repos.api_token.find_by_token(token_str);
        let key_fut = repos.api_key.find_by_presented(token_str);

        let (sync_result, api_result, key_result) = tokio::join!(sync_fut, api_fut, key_fut);

        if let Some(record) = sync_result.map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)? {
            if is_token_expired(record.expires_at) {
                return Err(StatusCode::FORBIDDEN);
            }

            if let Some(url_repo) = url_repo_id
                && record.repo_id != url_repo
            {
                return Err(StatusCode::FORBIDDEN);
            }

            ensure_active_user(repos, record.user_id).await?;

            let device = device_from_token(&record);
            return Ok(SyncAuth {
                user_id: record.user_id,
                repo_id: record.repo_id,
                credential: Credential::SyncToken,
                device,
            });
        }

        // A unified API key: it must carry the sync capability this request
        // needs, and cover the library in the URL.
        if let Some(lookup) = key_result.map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)? {
            if is_token_expired(lookup.key.expires_at) {
                return Err(StatusCode::FORBIDDEN);
            }
            let authority = KeyAuthority::from_lookup(&lookup).map_err(|error| {
                tracing::warn!(key_id = lookup.key.id, %error, "rejecting API key with an unreadable capability set");
                StatusCode::FORBIDDEN
            })?;
            let needed = if crate::domain::capability::requires_sync_write(method, path) {
                crate::domain::capability::Capability::SyncWrite
            } else {
                crate::domain::capability::Capability::SyncRead
            };
            // A sync client makes many small requests, so throttle this write.
            note_key_usage(repos, lookup.key.id).await;
            if !authority.has(needed) {
                return Err(StatusCode::FORBIDDEN);
            }
            if let Some(url_repo) = url_repo_id {
                if !authority.allows_repo(url_repo, needed.is_write()) {
                    return Err(StatusCode::FORBIDDEN);
                }
                // The key's scope is a ceiling, not a grant: the permission
                // check still decides, so a revoked account loses access at once.
                crate::domain::permission::check_repo_read_permission(
                    repos.member.as_ref(),
                    url_repo,
                    lookup.key.user_id,
                )
                .await
                .map_err(|_| StatusCode::FORBIDDEN)?;
            }

            ensure_active_user(repos, lookup.key.user_id).await?;

            return Ok(SyncAuth {
                user_id: lookup.key.user_id,
                repo_id: url_repo_id.unwrap_or("").to_string(),
                credential: Credential::Key(authority),
                device: None,
            });
        }

        // Fall back to a session API token — check expiration like AuthUser does.
        if let Some(record) = api_result.map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)? {
            if is_token_expired(record.expires_at) {
                return Err(StatusCode::FORBIDDEN);
            }
            // A 2FA pending token must not be usable as a full session.
            if record.is_pending {
                return Err(StatusCode::FORBIDDEN);
            }

            // Session tokens are not repo-scoped. On repo-scoped endpoints the
            // caller must be a member of the URL repo, closing the cross-repo
            // IDOR while keeping seaf-daemon's API-token logon working.
            if let Some(url_repo) = url_repo_id {
                crate::domain::permission::check_repo_read_permission(
                    repos.member.as_ref(),
                    url_repo,
                    record.user_id,
                )
                .await
                .map_err(|_| StatusCode::FORBIDDEN)?;
            }

            ensure_active_user(repos, record.user_id).await?;

            let device = device_from_api_token(&record);
            return Ok(SyncAuth {
                user_id: record.user_id,
                repo_id: url_repo_id.unwrap_or("").to_string(),
                credential: Credential::Session,
                device,
            });
        }

        Err(StatusCode::FORBIDDEN)
    }
}

/// Check if a token has expired: `expires_at` set and in the past.
fn is_token_expired(expires_at: Option<i64>) -> bool {
    matches!(expires_at, Some(exp) if chrono::Utc::now().timestamp() > exp)
}

/// Ensure the user behind a sync/API token still exists and is active, so a
/// deactivated or unverified account cannot keep syncing through a token issued
/// earlier.
async fn ensure_active_user(repos: &Repositories, user_id: i32) -> Result<(), StatusCode> {
    let user = repos
        .user
        .find_by_id(user_id)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        .ok_or(StatusCode::UNAUTHORIZED)?;
    if !user.is_active || !user.email_verified {
        return Err(StatusCode::FORBIDDEN);
    }
    Ok(())
}

/// Extract a sync/API token from headers: `Seafile-Repo-Token` first, then `Authorization`.
pub fn extract_sync_token(
    headers: &axum::http::HeaderMap,
) -> Result<String, base::error::AppError> {
    use base::error::AppError;

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

    // Missing credentials are malformed: seafile's fileserver answers 400 here.
    Err(AppError::BadRequest("token is null".into()))
}

/// Record that a key authenticated a request, at most once per
/// [`crate::repository::throttle::KEY_USAGE_WRITE_INTERVAL`]; best effort, and
/// it counts authentication rather than success.
async fn note_key_usage(repos: &Repositories, key_id: i32) {
    if !repos.key_usage_writes.allows_now(key_id) {
        return;
    }
    if let Err(error) = repos
        .api_key
        .touch_last_used(key_id, chrono::Utc::now().timestamp())
        .await
    {
        tracing::warn!(key_id, %error, "could not record API key usage");
    }
}

#[cfg(test)]
mod tests {
    use super::{AuthRejection, cookie_value, extract_url_repo_id, route_decision};
    use crate::domain::api_key::{KeyAuthority, RepoCeiling};
    use crate::domain::capability::{Capability, CapabilitySet};
    use crate::domain::credential::Credential;
    use axum::http::{HeaderMap, HeaderValue, Method, StatusCode};
    use axum::response::IntoResponse;
    use std::collections::HashMap;

    /// A rejected API request carries the same error document every other endpoint sends, so a
    /// client can show the reason instead of an empty body.
    #[tokio::test]
    async fn an_auth_rejection_carries_the_error_document() {
        let response = AuthRejection::from(StatusCode::UNAUTHORIZED).into_response();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        assert!(
            response
                .headers()
                .get(axum::http::header::CONTENT_TYPE)
                .and_then(|value| value.to_str().ok())
                .is_some_and(|value| value.starts_with("application/json")),
            "the rejection must be JSON"
        );
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let body: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert!(
            body["error_msg"].as_str().is_some_and(|m| !m.is_empty()),
            "a rejection must name the reason: {body:?}"
        );
    }

    /// The cookie parser finds the session cookie in the header shapes a browser actually sends,
    /// and still finds it next to a segment `Cookie::decode` rejects.
    #[test]
    fn the_session_cookie_is_found_whatever_the_header_shape() {
        let mut headers = HeaderMap::new();
        headers.insert(
            "cookie",
            HeaderValue::from_static("seahub-session=first; sfcsrftoken=x"),
        );
        assert_eq!(
            cookie_value(&headers, "seahub-session").as_deref(),
            Some("first")
        );

        // A segment without `=` makes the strict decoder reject the whole header; the scan still
        // finds the session cookie, so a malformed neighbour does not log the browser out.
        headers.insert(
            "cookie",
            HeaderValue::from_static("garbage; seahub-session=plain"),
        );
        assert_eq!(
            cookie_value(&headers, "seahub-session").as_deref(),
            Some("plain")
        );

        // Split across two headers, as a request that accumulated cookies can be.
        let mut headers = HeaderMap::new();
        headers.append("cookie", HeaderValue::from_static("a=1"));
        headers.append("cookie", HeaderValue::from_static("seahub-session=second"));
        assert_eq!(
            cookie_value(&headers, "seahub-session").as_deref(),
            Some("second")
        );

        // A cookie whose name merely ends with the one asked for is not a match.
        let mut headers = HeaderMap::new();
        headers.insert("cookie", HeaderValue::from_static("xseahub-session=nope"));
        assert_eq!(cookie_value(&headers, "seahub-session"), None);

        let mut headers = HeaderMap::new();
        headers.insert("cookie", HeaderValue::from_static("other=1"));
        assert_eq!(cookie_value(&headers, "seahub-session"), None);
    }

    const REPO: &str = "cfcab3e0-9eb4-4c4f-92d0-87db2cd8290d";
    const OTHER_REPO: &str = "11111111-1111-1111-1111-111111111111";

    /// A key carrying exactly `capabilities`, bound to `repos`.
    fn key(capabilities: &[Capability], repos: &[(&str, RepoCeiling)]) -> Credential {
        let mut set = CapabilitySet::EMPTY;
        for capability in capabilities {
            set.insert(*capability);
        }
        Credential::Key(KeyAuthority {
            key_id: 1,
            capabilities: set,
            all_repos: repos.is_empty(),
            repos: repos
                .iter()
                .map(|(id, ceiling)| ((*id).to_string(), *ceiling))
                .collect::<HashMap<_, _>>(),
        })
    }

    /// A session satisfies every capability, so classification must not block it.
    #[test]
    fn a_session_passes_every_classified_route() {
        for (method, route) in [
            (Method::GET, "/api2/repos/"),
            (Method::POST, "/api2/repos/"),
            (Method::DELETE, "/api2/repos/{repo_id}/"),
            (Method::GET, "/api2/repos/{repo_id}/dir/"),
            (Method::POST, "/api/v2.1/repos/{repo_id}/file/"),
            (Method::GET, "/api2/admin/users/"),
            // Session-only routes are exactly where a session is required.
            (Method::GET, "/api2/api-keys/"),
            (Method::POST, "/api2/api-keys/"),
        ] {
            assert_eq!(
                route_decision(
                    &Credential::Session,
                    &method,
                    route,
                    &route.replace("{repo_id}", REPO)
                ),
                Ok(()),
                "session must reach {method} {route}"
            );
        }
    }

    /// A route that nobody classified is denied to *every* credential, so adding
    /// an endpoint without classifying it cannot leave a silently open door.
    #[test]
    fn an_unclassified_route_is_denied_to_every_credential() {
        let session = Credential::Session;
        let key = key(&[Capability::FileRead], &[(REPO, RepoCeiling::Write)]);

        // Inside a classified namespace but absent from the table.
        for credential in [&session, &key] {
            assert_eq!(
                route_decision(
                    credential,
                    &Method::GET,
                    "/api2/not-a-route/",
                    "/api2/not-a-route/"
                ),
                Err(StatusCode::FORBIDDEN)
            );
        }
        // Outside every classified namespace: the fallback uses the raw URI,
        // which is what a request to an unknown path looks like.
        assert_eq!(
            route_decision(
                &session,
                &Method::GET,
                "/not-an-api/route",
                "/not-an-api/route"
            ),
            Err(StatusCode::FORBIDDEN)
        );
    }

    /// A denied request must leave a trace; the classified route in the same test
    /// is the control, since reaching it must not be reported as a gap.
    #[test]
    fn an_unclassified_route_is_recorded_and_a_classified_one_is_not() {
        use crate::middleware::route_audit;

        let gap = "/api2/control-gap-probe/";
        assert_eq!(
            route_decision(&Credential::Session, &Method::GET, gap, gap),
            Err(StatusCode::FORBIDDEN)
        );
        assert!(
            route_audit::recorded().contains(&("GET".to_string(), gap.to_string())),
            "the gap must be recorded so the end-to-end run can fail on it"
        );

        let classified = ("GET".to_string(), "/api2/repos/".to_string());
        assert_eq!(
            route_decision(
                &Credential::Session,
                &Method::GET,
                "/api2/repos/",
                "/api2/repos/"
            ),
            Ok(())
        );
        assert!(
            !route_audit::recorded().contains(&classified),
            "a classified route is not a gap"
        );
    }

    /// The key half of the policy: the route is matched on the template while the
    /// library ceiling is checked against the concrete id in the raw path.
    #[test]
    fn a_key_is_limited_by_its_capabilities_and_its_library_ceiling() {
        let read_only_here = key(
            &[Capability::FileRead, Capability::FileWrite],
            &[(REPO, RepoCeiling::Read)],
        );
        let template = "/api2/repos/{repo_id}/dir/";
        let uri = format!("/api2/repos/{REPO}/dir/");

        // A read goes through the read ceiling...
        assert_eq!(
            route_decision(&read_only_here, &Method::GET, template, &uri),
            Ok(())
        );
        // ...and a write does not.
        assert_eq!(
            route_decision(&read_only_here, &Method::POST, template, &uri),
            Err(StatusCode::FORBIDDEN),
            "a read ceiling must block the write"
        );

        // An unbound library is out of scope even though the capability is held.
        assert_eq!(
            route_decision(
                &read_only_here,
                &Method::GET,
                template,
                &format!("/api2/repos/{OTHER_REPO}/dir/")
            ),
            Err(StatusCode::FORBIDDEN)
        );

        // Key management stays closed to keys however wide they are.
        assert_eq!(
            route_decision(
                &read_only_here,
                &Method::GET,
                "/api2/api-keys/",
                "/api2/api-keys/"
            ),
            Err(StatusCode::FORBIDDEN)
        );
    }

    #[test]
    fn plain_repo_id_is_extracted() {
        assert_eq!(
            extract_url_repo_id(&format!("/seafhttp/repo/{REPO}/commit/HEAD")),
            Ok(Some(REPO.to_string()))
        );
        // The nested router strips the prefix, so the handler-side form works too.
        assert_eq!(
            extract_url_repo_id(&format!("/{REPO}/commit/HEAD")),
            Ok(Some(REPO.to_string()))
        );
        // Trailing slash variants.
        assert_eq!(
            extract_url_repo_id(&format!("/seafhttp/repo/{REPO}")),
            Ok(Some(REPO.to_string()))
        );
    }

    #[test]
    fn endpoints_without_a_repo_segment_return_none() {
        // `nest` strips the prefix, so the middleware sees `/accessible-repos`
        // for `/seafhttp/accessible-repos`; only the allow-list may skip the
        // binding check.
        for path in [
            "/accessible-repos",
            "/head-commits-multi/",
            "/protocol-version",
        ] {
            assert_eq!(extract_url_repo_id(path), Ok(None), "path={path}");
        }
    }

    /// Any other path that does not yield a UUID must fail closed rather than
    /// being treated as "this endpoint has no repo".
    #[test]
    fn unknown_non_uuid_paths_fail_closed() {
        for path in ["/whatever/commit/HEAD", "/seafhttp/whatever", "/not-a-uuid"] {
            assert_eq!(extract_url_repo_id(path), Err(()), "path={path}");
        }
    }

    /// Regression: the binding value must be derived by percent-decoding, as
    /// axum's `Path` does. The bug compared the *undecoded* segment, so a `%XX`
    /// id failed the UUID test and skipped the binding check entirely.
    #[test]
    fn percent_encoded_repo_id_is_decoded_before_the_binding_check() {
        // '%63' == 'c' — the exact encoding shape used in the PoC.
        let encoded = format!("%63{}", &REPO[1..]);
        assert_ne!(encoded, REPO);
        assert_eq!(
            extract_url_repo_id(&format!("/seafhttp/repo/{encoded}/commit/HEAD")),
            Ok(Some(REPO.to_string())),
            "the decoded value must be used for the binding comparison"
        );
        // Every hex char must decode consistently, in both path forms.
        for (i, ch) in REPO.char_indices().filter(|(_, c)| c.is_ascii_hexdigit()) {
            let mut buf = String::new();
            buf.push_str(&REPO[..i]);
            buf.push_str(&format!("%{:02x}", ch as u8));
            buf.push_str(&REPO[i + ch.len_utf8()..]);
            for path in [
                format!("/seafhttp/repo/{buf}/commit/HEAD"),
                format!("/{buf}/commit/HEAD"),
            ] {
                assert_eq!(
                    extract_url_repo_id(&path),
                    Ok(Some(REPO.to_string())),
                    "encoded char {ch} at {i} must decode to the real repo id (path={path})"
                );
            }
        }
    }

    /// Regression: double encoding (`%2563` → `%63`) must also fail.
    #[test]
    fn double_encoded_repo_id_is_rejected() {
        // One decode yields "…%63…", which is not a UUID ⇒ fail closed.
        let encoded = format!("%2563{}", &REPO[1..]);
        assert_eq!(
            extract_url_repo_id(&format!("/seafhttp/repo/{encoded}/commit/HEAD")),
            Err(())
        );
    }

    #[test]
    fn malformed_segments_fail_closed() {
        for path in [
            "/seafhttp/repo/not-a-uuid/commit/HEAD",
            "/seafhttp/repo/../../etc/commit/HEAD",
            "/seafhttp/repo/cfcab3e0-9eb4-4c4f-92d0-87db2cd8290/commit/HEAD", // 35 chars
            "/seafhttp/repo/cfcab3e0_9eb4_4c4f_92d0_87db2cd8290d/commit/HEAD",
            // Encoded non-UUID: decodes to something that is still not a UUID.
            "/seafhttp/repo/%6eot-a-uuid/commit/HEAD",
        ] {
            assert_eq!(extract_url_repo_id(path), Err(()), "path={path}");
        }
    }

    #[test]
    fn uppercase_uuid_is_accepted() {
        let upper = REPO.to_uppercase();
        assert_eq!(
            extract_url_repo_id(&format!("/seafhttp/repo/{upper}/commit/HEAD")),
            Ok(Some(upper))
        );
    }
}
