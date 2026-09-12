use axum::{
    extract::FromRequestParts,
    http::{StatusCode, request::Parts},
};
use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant};

use crate::AppState;
use crate::domain::api_key::KeyAuthority;
use crate::domain::capability::{RouteAccess, required_access};
use crate::domain::credential::Credential;
use crate::domain::permission::RepoScope;
use crate::domain::repo_path;
use crate::repository::Repositories;
use base::error::AppError;

#[derive(Debug, Clone)]
pub struct AuthUser {
    pub user_id: i32,
    pub email: String,
    /// What authenticated the request.
    ///
    /// Never [`Credential::SyncToken`]: repository-scoped tokens are accepted
    /// only by the sync protocol (see [`SyncAuth`]).
    pub credential: Credential,
}

impl AuthUser {
    /// The libraries an account-wide query may return.
    pub fn repo_scope(&self) -> RepoScope {
        self.credential.repo_scope()
    }

    /// Enforce a key's library scope for a repo id the path guard cannot see.
    ///
    /// Some endpoints carry the library in a query parameter or a JSON body
    /// (`repo-tokens`, the batch copy/move handlers, `search-file`), so callers
    /// apply this next to the membership check for those ids. Sessions are
    /// unaffected: they carry no ceiling.
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
}

impl SyncAuth {
    /// The libraries an account-wide sync query may return.
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
        // skip the repo binding check. A repo segment that is present but is not
        // a valid UUID is rejected outright (see `extract_url_repo_id`) — axum's
        // `Path` extractor hands the *decoded* value to the handler, so treating
        // an undecodable segment as "not a repo endpoint" would let a
        // percent-encoded id (`%63cab3e0-…`) bypass the binding check entirely.
        let url_repo_id =
            extract_url_repo_id(parts.uri.path()).map_err(|_| base::error::AppError::Forbidden)?;

        // First try to authenticate via sync token (primary sync protocol path).
        // We look up sync_tokens directly here (rather than delegating to from_token)
        // so we can capture client_id from the request URI and store peer info.
        let sync_record = repos.sync_token.find_by_token(&token).await?;

        if let Some(record) = sync_record {
            // Check token expiry / repository binding. seaf-daemon only
            // understands 403 (permission) and 400 (malformed request), so an
            // invalid or cross-repo repo token is rejected as 403 — matching
            // seafile's fileserver (`server/http-server.c`, `fileserver/sync_api.go`).
            if is_token_expired(record.expires_at) {
                return Err(base::error::AppError::Forbidden);
            }

            // A sync token is bound to exactly one repo: it must match the repo
            // in the URL (matches seafile-server's validate_token behaviour).
            if let Some(url_repo) = &url_repo_id
                && record.repo_id != *url_repo
            {
                return Err(base::error::AppError::Forbidden);
            }

            let user_id = record.user_id;
            let repo_id = record.repo_id.clone();

            // A deactivated account must not keep syncing (AuthUser applies the
            // same is_active check on the web/API surfaces).
            if !repos
                .user
                .find_by_id(user_id)
                .await?
                .is_some_and(|u| u.is_active)
            {
                return Err(base::error::AppError::Forbidden);
            }

            // Capture client_id, client_name, client_ver from URL query params
            // and update the sync_token's peer info. This mirrors seafile-server's
            // RepoTokenPeerInfo table for device linking.
            if let Some(query) = parts.uri.query()
                && let Ok(params) =
                    serde_urlencoded::from_str::<std::collections::HashMap<String, String>>(query)
                && let Some(client_id) = params.get("client_id")
                && should_write_peer_info(record.id)
            {
                let now = chrono::Utc::now().timestamp();
                let peer_ip = parts
                    .extensions
                    .get::<axum::extract::ConnectInfo<std::net::SocketAddr>>()
                    .map(|ci| {
                        crate::middleware::effective_client_ip(
                            &ci.0,
                            &parts.headers,
                            &state.config.server.trusted_proxies,
                        )
                    });
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

            return Ok(SyncAuth {
                user_id,
                repo_id,
                credential: Credential::SyncToken,
            });
        }

        // Fall back to a unified API key or a session API token.
        SyncAuth::from_token(
            repos,
            &token,
            url_repo_id.as_deref(),
            &parts.method,
            parts.uri.path(),
        )
        .await
        .map_err(|_| base::error::AppError::Forbidden)
    }
}

/// Sync-protocol endpoints that live under `/seafhttp/` but carry no repo id.
///
/// These are the only paths allowed to skip the token↔repo binding check.
const NON_REPO_SYNC_ENDPOINTS: [&str; 3] =
    ["accessible-repos", "head-commits-multi", "protocol-version"];

/// Extract the repo id from a `/seafhttp/repo/{repo_id}/…` URL path.
///
/// `nest` strips the matched prefix before the request reaches the inner
/// router, and `SyncAuth` can be extracted at several nesting depths, so the
/// path may arrive as either the full form (`/seafhttp/repo/{id}/…`) or the
/// stripped form (`/{id}/…`). Both are accepted.
///
/// # Security
///
/// The path is percent-decoded **before** the UUID check, because axum's `Path`
/// extractor also decodes it. Without this, `/seafhttp/repo/%63cab3e0-…/commit`
/// would look like a non-UUID segment (→ `None` → binding check skipped) while
/// the handler would see the victim's real repo id — letting any sync token
/// read any repo.
///
/// The check is fail-closed: only the explicit [`NON_REPO_SYNC_ENDPOINTS`]
/// allow-list may return `Ok(None)`; anything else that fails to yield a UUID
/// is `Err`, so a malformed or encoded repo segment can never be mistaken for
/// "this endpoint has no repo".
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

        // Only account credentials authenticate `/api2/*` and the Web UI:
        // unified API keys (created in the settings UI) and session API tokens.
        // Repository-scoped sync tokens are accepted exclusively by [`SyncAuth`]
        // on `/seafhttp/...`, mirroring seahub's `TokenAuthentication`, which
        // never consults the repo-token table. Treating a repo token as an
        // account session would let a leaked library credential read the whole
        // account.
        //
        // A key and a session token can never share a value (both are 160-bit
        // random), so the two lookups run together and the order is immaterial.
        let (key_lookup, api_record) = tokio::join!(
            repos.api_key.find_by_presented(&token_str),
            repos.api_token.find_by_token(&token_str),
        );
        let key_lookup = key_lookup.map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
        let api_record = api_record.map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

        let (user_id, credential) = if let Some(lookup) = key_lookup {
            if is_token_expired(lookup.key.expires_at) {
                return Err(StatusCode::UNAUTHORIZED);
            }
            let authority = KeyAuthority::from_lookup(&lookup).map_err(|error| {
                // An unreadable grant means this build cannot honour what the
                // row promises; refusing the credential is the safe direction.
                tracing::warn!(key_id = lookup.key.id, %error, "rejecting API key with an unreadable capability set");
                StatusCode::UNAUTHORIZED
            })?;
            let credential = Credential::Key(authority);
            enforce_route_access(&credential, parts)?;
            (lookup.key.user_id, credential)
        } else if let Some(token_record) = api_record {
            // Check API token expiration.
            if is_token_expired(token_record.expires_at) {
                return Err(StatusCode::UNAUTHORIZED);
            }
            // A 2FA pending token must not be usable as a full session.
            if token_record.is_pending {
                return Err(StatusCode::UNAUTHORIZED);
            }
            (token_record.user_id, Credential::Session)
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
            credential,
        })
    }
}

/// Apply a credential's capability and library limits to a request.
///
/// Sessions never reach this: only key callers are classified, so the behaviour
/// of a login token is unchanged. An unclassified route is denied, which is what
/// makes a newly added endpoint fail closed instead of silently accepting every
/// key.
///
/// Classification uses the router's matched path template, not the raw URI:
/// `nest` strips the matched prefix before the request reaches a nested handler,
/// so the raw path is `/{repo_id}/dir/` where the template is
/// `/api2/repos/{repo_id}/dir/`. Library binding is checked against the raw path
/// instead, because that is where the concrete id lives.
fn enforce_route_access(credential: &Credential, parts: &Parts) -> Result<(), StatusCode> {
    let matched = parts
        .extensions
        .get::<axum::extract::MatchedPath>()
        .map(axum::extract::MatchedPath::as_str);
    let route = matched.unwrap_or_else(|| parts.uri.path());

    let Some(access) = required_access(&parts.method, route) else {
        tracing::warn!(
            method = %parts.method,
            path = parts.uri.path(),
            credential = credential.kind_id(),
            "credential used on a route with no classification"
        );
        return Err(StatusCode::FORBIDDEN);
    };
    if !credential.allows_route(access) {
        return Err(StatusCode::FORBIDDEN);
    }
    // A key carries a per-library ceiling on top of the route's requirement.
    // Every other credential is the account itself and has no ceiling, so
    // `allows_repo` accepts unconditionally there.
    if let RouteAccess::Capability(capability) = access
        && let Some(repo_id) = repo_path::find_repo_id(parts.uri.path())
        && !credential.allows_repo(&repo_id, capability.is_write())
    {
        return Err(StatusCode::FORBIDDEN);
    }
    Ok(())
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
        && !crate::service::auth::csrf::validate_csrf_header(
            headers,
            &state.csrf_secret,
            session_token,
        )
    {
        return None;
    }

    Some(session_token.to_string())
}

impl SyncAuth {
    /// Authenticate via sync token or API token.
    ///
    /// `url_repo_id` is the repo in the request URL path (if any). Sync tokens
    /// must match it; API tokens are only accepted when the caller is a member
    /// of that repo.
    pub async fn from_token(
        repos: &Repositories,
        token_str: &str,
        url_repo_id: Option<&str>,
        method: &axum::http::Method,
        path: &str,
    ) -> Result<Self, StatusCode> {
        // Query all three credential tables concurrently.
        let sync_fut = repos.sync_token.find_by_token(token_str);
        let api_fut = repos.api_token.find_by_token(token_str);
        let key_fut = repos.api_key.find_by_presented(token_str);

        let (sync_result, api_result, key_result) = tokio::join!(sync_fut, api_fut, key_fut);

        // Check sync token first (has repo_id — preferred).
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

            return Ok(SyncAuth {
                user_id: record.user_id,
                repo_id: record.repo_id,
                credential: Credential::SyncToken,
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
            if !authority.has(needed) {
                return Err(StatusCode::FORBIDDEN);
            }
            if let Some(url_repo) = url_repo_id {
                if !authority.allows_repo(url_repo, needed.is_write()) {
                    return Err(StatusCode::FORBIDDEN);
                }
                // The key's scope is a ceiling, not a grant: membership still
                // decides, so a removed collaborator loses access at once.
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
            // IDOR while keeping seaf-daemon's API-token logon working for the
            // user's own repos.
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

            return Ok(SyncAuth {
                user_id: record.user_id,
                repo_id: url_repo_id.unwrap_or("").to_string(),
                credential: Credential::Session,
            });
        }

        Err(StatusCode::FORBIDDEN)
    }
}

/// Check if a token has expired. Returns `true` when `expires_at` is set
/// and is in the past.
fn is_token_expired(expires_at: Option<i64>) -> bool {
    matches!(expires_at, Some(exp) if chrono::Utc::now().timestamp() > exp)
}

/// Ensure the user behind a sync/API token still exists and is active.
///
/// Mirrors the `is_active` check that [`AuthUser`] applies on the web/API
/// surfaces, so a deactivated account cannot keep syncing through a token
/// that was issued before deactivation.
async fn ensure_active_user(repos: &Repositories, user_id: i32) -> Result<(), StatusCode> {
    let user = repos
        .user
        .find_by_id(user_id)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        .ok_or(StatusCode::UNAUTHORIZED)?;
    if !user.is_active {
        return Err(StatusCode::FORBIDDEN);
    }
    Ok(())
}

/// Extract a sync/API token from HTTP headers. Used by /seafhttp/ endpoints.
/// Checks the Seafile-Repo-Token header first, then the Authorization header.
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

    // Missing credentials are a malformed request: seafile's fileserver answers
    // 400 here, and seaf-daemon classifies 400 as a general request error.
    Err(AppError::BadRequest("token is null".into()))
}

/// How often a sync token's peer info may be persisted to the DB at most.
/// A seafile client that carries `client_id` on every `/seafhttp/` request
/// would otherwise turn each request into a write; SQLite writes serialize,
/// so this bounds the cost while keeping `last_sync_time` within ~1 minute of
/// accuracy.
const PEER_INFO_WRITE_INTERVAL: Duration = Duration::from_secs(60);

/// Last persisted timestamp per sync-token id, so the write throttle above has
/// shared state across requests. Entries older than the interval are pruned on
/// insert, so the map stays bounded to tokens seen in the last minute.
static PEER_INFO_LAST_WRITE: LazyLock<Mutex<HashMap<i32, Instant>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// Whether peer info for `token_id` may be written now (throttled to at most
/// once per [`PEER_INFO_WRITE_INTERVAL`]).
fn should_write_peer_info(token_id: i32) -> bool {
    let mut last_writes = PEER_INFO_LAST_WRITE
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let now = Instant::now();
    if last_writes
        .get(&token_id)
        .is_some_and(|&last| now.duration_since(last) < PEER_INFO_WRITE_INTERVAL)
    {
        return false;
    }
    // Prune entries that have aged out of the interval.
    last_writes.retain(|_, last| now.duration_since(*last) < PEER_INFO_WRITE_INTERVAL);
    last_writes.insert(token_id, now);
    true
}

#[cfg(test)]
mod tests {
    use super::extract_url_repo_id;

    const REPO: &str = "cfcab3e0-9eb4-4c4f-92d0-87db2cd8290d";

    #[test]
    fn plain_repo_id_is_extracted() {
        assert_eq!(
            extract_url_repo_id(&format!("/seafhttp/repo/{REPO}/commit/HEAD")),
            Ok(Some(REPO.to_string()))
        );
        // The nested router strips the prefix, so the handler-side form must
        // work too.
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
        // `nest` strips the matched prefix before the request reaches the inner
        // router, so the middleware observes the stripped form for the
        // non-repo protocol endpoints (`/seafhttp/accessible-repos` arrives as
        // `/accessible-repos`). These are the only paths allowed to skip the
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

    /// Regression: the middleware must derive the binding value the same
    /// way axum's `Path` extractor does — by percent-decoding. Decoding is the
    /// correct behaviour (it makes the compared value identical to the one the
    /// handler uses); the vulnerability was that the *undecoded* segment was
    /// compared, so a `%XX`-encoded id failed the UUID test and skipped the
    /// binding check entirely.
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
        // Every hex char is encodable — all of them must decode consistently,
        // in both the full and the stripped path form.
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
