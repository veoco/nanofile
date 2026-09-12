use askama::Template;
use axum::{
    extract::{ConnectInfo, Path, State},
    http::{HeaderMap, StatusCode},
    response::{Html, IntoResponse, Response},
};
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;

use crate::AppState;
use crate::i18n::I18n;
use base::error::AppError;

// ── Templates ─────────────────────────────────────────────────────────────

#[allow(dead_code)]
#[derive(Template)]
#[template(path = "web/upload_link_view.html")]
struct UploadLinkViewTemplate {
    pub t: &'static I18n,
    pub urls: &'static crate::static_assets::TemplateUrls,
    pub token: String,
    pub repo_id: String,
    pub path: String,
    pub dir_name: String,
    pub has_password: bool,
    pub max_upload_size_mb: i64,
    pub description: Option<String>,
}

#[allow(dead_code)]
#[derive(Template)]
#[template(path = "web/share_access_validation.html")]
struct ShareAccessValidationTemplate {
    pub t: &'static I18n,
    pub token: String,
    pub error: Option<String>,
    pub form_action: String,
}

// ── Helpers ───────────────────────────────────────────────────────────────

/// Cookie marking that this browser session has supplied the correct
/// upload-link password.
///
/// Delegates to the shared link-unlock helper so the upload-link and
/// share-link paths cannot drift: both sign an HMAC over the token (a
/// plaintext `1` is rejected) and both gain the `Secure` attribute when
/// `server.site_url` is HTTPS — a cookie that unlocks a password-protected
/// link must never travel over plaintext.
fn upload_link_cookie(state: &AppState, token: &str) -> String {
    use crate::service::auth::csrf::{link_unlock_cookie, upload_link_cookie_name};
    link_unlock_cookie(
        &upload_link_cookie_name(token),
        token,
        &state.csrf_secret,
        state.config.server.secure_cookies(),
    )
}

/// Validate the upload link: check it exists, not expired, repo exists.
async fn validate_upload_link(
    state: &Arc<AppState>,
    token: &str,
) -> Result<infra::entity::upload_link::Model, AppError> {
    let link = state
        .repos
        .upload_link
        .find_by_token(token)
        .await?
        .ok_or_else(|| AppError::NotFound("Upload link not found".into()))?;

    // Check expiry
    if let Some(exp) = link.expires_at
        && chrono::Utc::now().timestamp() > exp
    {
        return Err(AppError::NotFound("Upload link has expired".into()));
    }

    // Check repo exists
    if !state.sync_service().repo_exists(&link.repo_id).await? {
        return Err(AppError::NotFound("Upload link not found".into()));
    }

    // The link acts as its creator, so it must stop resolving once the creator
    // loses access to the library (member removed, account deactivated). The
    // actual upload path re-checks write permission on the token, but the view
    // page itself lists directory entries and must not outlive access either.
    if !crate::service::sharing::share::link_creator_may_access(
        &state.repos,
        link.creator_id,
        &link.repo_id,
        false,
    )
    .await
    {
        return Err(AppError::NotFound("Upload link not found".into()));
    }

    Ok(link)
}

/// Check whether the supplied password matches the stored hash.
///
/// The password arrives either in the `X-Seafile-Sharelink-Password` header or
/// as a form field on the POST; it is deliberately never read from the query
/// string, which would put it into browser history, referrers and request logs.
async fn check_password(
    link: &infra::entity::upload_link::Model,
    provided: Option<&str>,
    password_hash_iterations: u32,
) -> bool {
    let stored_hash = match link.password {
        Some(ref h) => h,
        None => return true, // no password required
    };

    match provided {
        Some(pwd) => {
            crate::service::auth::password::verify_password_async(
                pwd.to_string(),
                stored_hash.clone(),
                password_hash_iterations,
            )
            .await
        }
        None => false,
    }
}

// ── Main GET handler ──────────────────────────────────────────────────────

/// GET /u/{token}/ — show the public upload page.
///
/// Query parameters are deliberately not extracted: the password must arrive
/// via the `X-Seafile-Sharelink-Password` header, the password form POST, or
/// the signed unlock cookie — never the URL.
pub async fn upload_link_view(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(token): Path<String>,
) -> Result<Response, AppError> {
    crate::middleware::ensure_share_links_enabled(&state)?;
    let link = validate_upload_link(&state, &token).await?;

    // Password check: header or signed cookie only, never the query string.
    let provided_pwd = headers
        .get("X-Seafile-Sharelink-Password")
        .and_then(|v| v.to_str().ok());
    let pw_ok = check_password(
        &link,
        provided_pwd,
        state.config.auth.password_hash_iterations,
    )
    .await;
    // The password form POST sets `visited_ufs_{token}`; accept that cookie as
    // an unlock too, so the redirect back to /u/{token}/ isn't bounced to the
    // form again.
    let cookie_ok = link.password.is_some()
        && crate::service::auth::csrf::has_valid_upload_link_cookie(
            headers.get("cookie").and_then(|v| v.to_str().ok()),
            &token,
            &state.csrf_secret,
        );
    let unlocked = pw_ok || cookie_ok;

    // If password is required but not satisfied, show password form
    if !unlocked {
        // Throttle wrong-password attempts here too: the POST handler has an
        // IP-keyed limiter, but this GET path can also be handed a password
        // directly and would otherwise be an unthrottled way to test guesses.
        if provided_pwd.is_some() {
            super::share_view::record_link_password_failure(&state, &token)?;
        }
        let error = if provided_pwd.is_some() {
            Some("Incorrect password".to_string())
        } else {
            None
        };
        let tpl = ShareAccessValidationTemplate {
            t: I18n::from_headers(&headers, &state.config.ui.default_language),
            token: token.clone(),
            error,
            form_action: format!("/u/{}/", token),
        };
        let html = tpl
            .render()
            .map_err(|e| AppError::Internal(e.to_string()))?;
        return Ok(Html(html).into_response());
    }

    // Build template data
    let dir_name = link
        .path
        .trim_end_matches('/')
        .rsplit_once('/')
        .map(|(_, n)| n.to_string())
        .unwrap_or_else(|| link.path.clone());

    let tpl = UploadLinkViewTemplate {
        t: I18n::from_headers(&headers, &state.config.ui.default_language),
        urls: crate::static_assets::template_urls(),
        token: link.token.clone(),
        repo_id: link.repo_id.clone(),
        path: link.path.clone(),
        dir_name,
        has_password: link.password.is_some(),
        max_upload_size_mb: state.config.server.max_upload_size_mb as i64,
        description: link.description.clone(),
    };

    let html = tpl
        .render()
        .map_err(|e| AppError::Internal(e.to_string()))?;

    // Valid password (provided or previously verified) → mark this session as
    // authorized so the upload URL API grants a token.
    let mut resp = Html(html).into_response();
    if link.password.is_some()
        && pw_ok
        && let Ok(value) = axum::http::HeaderValue::from_str(&upload_link_cookie(&state, &token))
    {
        resp.headers_mut()
            .append(axum::http::header::SET_COOKIE, value);
    }
    Ok(resp)
}

// ── POST handler for password submission ──────────────────────────────────

/// POST /u/{token}/ — validate the password, set a session flag cookie, then
/// redirect to the upload page (the password is NOT carried in the URL).
pub async fn upload_link_view_post(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    Path(token): Path<String>,
    axum::Form(form): axum::Form<HashMap<String, String>>,
) -> Result<Response, AppError> {
    crate::middleware::ensure_share_links_enabled(&state)?;
    let link = validate_upload_link(&state, &token).await?;

    let password = form
        .get("password")
        .ok_or_else(|| AppError::BadRequest("password required".into()))?;

    // Rate limit password attempts per client IP.
    let client_ip = crate::middleware::effective_client_ip(
        &addr,
        &headers,
        &state.config.server.trusted_proxies,
    );
    let rl_key = format!("link_password:{client_ip}");
    if state.auth_limiters.link_password.is_limited(&rl_key) {
        return Err(AppError::TooManyRequests);
    }
    state.auth_limiters.link_password.record_attempt(&rl_key);

    let valid = crate::service::auth::password::verify_password_async(
        password.clone(),
        link.password.clone().unwrap_or_default(),
        state.config.auth.password_hash_iterations,
    )
    .await;

    if !valid {
        // Same per-token cap as the share-link POST path: the IP limiter alone
        // does not bound a distributed brute force against one link token.
        crate::handler::web::share_view::record_link_password_failure(&state, &token)?;
        let tpl = ShareAccessValidationTemplate {
            t: I18n::from_headers(&headers, &state.config.ui.default_language),
            token: token.clone(),
            error: Some("Incorrect password".to_string()),
            form_action: format!("/u/{}/", token),
        };
        let html = tpl
            .render()
            .map_err(|e| AppError::Internal(e.to_string()))?;
        return Ok(Html(html).into_response());
    }

    let mut resp = (StatusCode::FOUND, [("Location", format!("/u/{}/", token))]).into_response();
    if let Ok(value) = axum::http::HeaderValue::from_str(&upload_link_cookie(&state, &token)) {
        resp.headers_mut()
            .append(axum::http::header::SET_COOKIE, value);
    }
    Ok(resp)
}
