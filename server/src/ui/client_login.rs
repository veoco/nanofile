use askama::Template;
/// GET /client-login/ — auto-login from a one-time token minted by
/// POST /api2/client-login/; the sync client opens the browser here.
use axum::{
    extract::{ConnectInfo, Query, State},
    http::{HeaderMap, StatusCode},
    response::{Html, IntoResponse, Redirect},
};
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant};

use crate::AppState;
use crate::domain::session_source::SessionSource;
use crate::i18n::I18n;
use crate::repository::api_token::CreateSessionTokenParams;
use crate::service::auth::token::generate_api_token;
use base::error::AppError;

const TOKEN_TTL_SECS: i64 = 30;

/// How long a token stays bound to the requesting address: slightly longer than
/// the token, so a slow browser launch is not penalised.
const ISSUER_TTL: Duration = Duration::from_secs(60);

/// Hard cap on the issuer map, so a flood of tokens cannot grow it forever.
const MAX_ISSUERS: usize = 10_000;

/// `token → (issuing client address, issued at)`, in memory only: the token
/// lives 30 seconds, so there is nothing worth persisting. In a multi-instance
/// deployment the lookup may miss, which only means the confirmation page.
static ISSUERS: LazyLock<Mutex<HashMap<String, (String, Instant)>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// Remember which client address requested `token` (called by the API handler).
pub fn remember_issuer(token: &str, client_ip: &str) {
    let mut issuers = ISSUERS.lock().unwrap_or_else(|e| e.into_inner());
    issuers.retain(|_, (_, issued_at)| issued_at.elapsed() < ISSUER_TTL);
    if issuers.len() >= MAX_ISSUERS {
        issuers.clear();
    }
    issuers.insert(token.to_string(), (client_ip.to_string(), Instant::now()));
}

fn issuer_matches(token: &str, client_ip: &str) -> bool {
    let issuers = ISSUERS.lock().unwrap_or_else(|e| e.into_inner());
    issuers
        .get(token)
        .is_some_and(|(ip, issued_at)| issued_at.elapsed() < ISSUER_TTL && ip == client_ip)
}

/// Confirmation page shown when the browser cannot be proven to be the token's issuer.
#[derive(Template)]
#[template(path = "web/client_login_confirm.html")]
struct ClientLoginConfirmTemplate {
    pub t: &'static I18n,
    pub urls: &'static crate::static_assets::TemplateUrls,
    /// The one-time token being confirmed (echoed into the form).
    pub token: String,
    /// Optional post-login destination, preserved across the confirmation.
    pub next: Option<String>,
}

pub async fn client_token_login(
    State(state): State<Arc<AppState>>,
    ConnectInfo(addr): ConnectInfo<std::net::SocketAddr>,
    headers: axum::http::HeaderMap,
    Query(params): Query<HashMap<String, String>>,
) -> Result<impl IntoResponse, AppError> {
    let token_str = match params.get("token") {
        Some(t) if t.len() == 32 => t.clone(),
        _ => return Ok(Redirect::to("/libraries/").into_response()),
    };

    // This handler signs the browser in as the token's owner, on a top-level GET.
    // `validate_origin` alone is not enough: it accepts a request with *neither*
    // `Origin` nor `Referer`, exactly what a referrer-suppressed cross-site link
    // produces, letting an attacker mint a token for their own account and
    // silently switch a victim's browser to it.
    //
    // The issuing client address is therefore the only signal that decides a
    // silent login: the client and the browser it opens share an egress address.
    // `Sec-Fetch-Site` grants no trust — `none` is also what a link from chat,
    // mail or a PDF produces — so it can only force the confirmation page, whose
    // POST carries an `Origin` the attacker cannot suppress.
    let client_ip = crate::middleware::effective_client_ip(
        &addr,
        &headers,
        &state.config().server.trusted_proxies,
    );
    let cross_site_navigation = headers
        .get("sec-fetch-site")
        .and_then(|v| v.to_str().ok())
        .is_some_and(|site| !matches!(site, "none" | "same-origin"));
    let trusted = !cross_site_navigation && issuer_matches(&token_str, &client_ip);

    if !trusted {
        let allowed_origin = state.config().server.site_url_origin();
        let html = ClientLoginConfirmTemplate {
            t: I18n::from_headers(&headers, &state.config().ui.default_language),
            urls: crate::static_assets::template_urls(),
            token: token_str.clone(),
            next: params
                .get("next")
                .filter(|n| crate::ui::client_login::resolve_next(Some(n), &allowed_origin) == **n)
                .cloned(),
        }
        .render()
        .map_err(|e| AppError::Internal(e.to_string()))?;
        return Ok(Html(html).into_response());
    }

    complete_login(
        &state,
        &token_str,
        params.get("next").map(String::as_str),
        crate::ui::user_agent::from_headers(&headers),
    )
    .await
}

/// POST /client-login/ — the confirmation form rendered above. A cross-site form
/// POST always carries `Origin`, so `validate_origin` is a real control here.
pub async fn client_token_login_confirm(
    State(state): State<Arc<AppState>>,
    headers: axum::http::HeaderMap,
    axum::Form(form): axum::Form<HashMap<String, String>>,
) -> Result<impl IntoResponse, AppError> {
    let origin = state.config().server.site_url_origin();
    if !crate::service::auth::csrf::validate_origin(&headers, &origin) {
        return Ok(Redirect::to("/libraries/").into_response());
    }

    let token_str = match form.get("token") {
        Some(t) if t.len() == 32 => t.clone(),
        _ => return Ok(Redirect::to("/libraries/").into_response()),
    };

    complete_login(
        &state,
        &token_str,
        form.get("next").map(String::as_str),
        crate::ui::user_agent::from_headers(&headers),
    )
    .await
}

/// Exchange a one-time client-login token for a browser session.
async fn complete_login(
    state: &Arc<AppState>,
    token_str: &str,
    next: Option<&str>,
    // The browser's `User-Agent`; this token has no device fields, so it identifies the session.
    user_agent: Option<String>,
) -> Result<axum::response::Response, AppError> {
    let allowed_origin = state.config().server.site_url_origin();
    let record = state
        .repos
        .client_login_token
        .find_by_token(token_str)
        .await
        .map_err(|_| AppError::internal("database error"))?;

    let record = match record {
        Some(r) => r,
        None => {
            let next = resolve_next(next, &allowed_origin);
            return Ok(Redirect::to(&next).into_response());
        }
    };

    // Check 30-second expiry
    let now = chrono::Utc::now().timestamp();
    let elapsed = now - record.created_at;
    if !(0..=TOKEN_TTL_SECS).contains(&elapsed) {
        let _ = state.repos.client_login_token.delete(record).await;
        let next = resolve_next(next, &allowed_origin);
        return Ok(Redirect::to(&next).into_response());
    }

    // Token is valid — get the username before deleting
    let username = record.username.clone();

    // Delete the token (one-time use)
    let _ = state.repos.client_login_token.delete(record).await;

    let user_record = state
        .repos
        .user
        .find_by_email(&username)
        .await
        .map_err(|_| AppError::internal("database error"))?;

    let user_record = match user_record {
        Some(u) if u.is_active && u.email_verified => u,
        _ => {
            let next = resolve_next(next, &allowed_origin);
            return Ok(Redirect::to(&next).into_response());
        }
    };

    let api_token = generate_api_token();
    let ttl_days = state.config().auth.api_token_ttl_days;
    let expires_at = now + (ttl_days as i64 * 86400);

    state
        .repos
        .api_token
        .create_session_token(CreateSessionTokenParams {
            user_id: user_record.id,
            token: api_token.clone(),
            created_at: now,
            expires_at: Some(expires_at),
            device_id: None,
            platform: None,
            device_name: None,
            client_version: None,
            is_pending: false,
            // The desktop client opened this in a browser and it sets the session
            // cookie, but the user did not sign in to get it — hence the label.
            source: SessionSource::WebClientLogin,
            user_agent,
        })
        .await
        .map_err(|e| AppError::internal(format!("failed to create session token: {e}")))?;

    let secure = if state.config().server.secure_cookies() {
        "; Secure"
    } else {
        ""
    };
    let cookie = format!(
        "seahub-session={}; HttpOnly; SameSite=Lax; Path=/; Max-Age={}{}",
        api_token,
        ttl_days * 86400,
        secure,
    );

    let next = resolve_next(next, &allowed_origin);

    let csrf_cookie = crate::service::auth::csrf::csrf_cookie_header(
        &state.csrf_secret,
        &api_token,
        state.config().server.secure_cookies(),
        Some(ttl_days * 86400),
    );

    let mut resp_headers = HeaderMap::new();
    resp_headers.insert(
        axum::http::header::LOCATION,
        axum::http::HeaderValue::from_str(&next)
            .map_err(|_| AppError::internal("Failed to create location header"))?,
    );
    resp_headers.append(
        axum::http::header::SET_COOKIE,
        cookie
            .parse::<axum::http::HeaderValue>()
            .map_err(|_| AppError::internal("Failed to create session cookie header"))?,
    );
    resp_headers.append(
        axum::http::header::SET_COOKIE,
        csrf_cookie
            .parse::<axum::http::HeaderValue>()
            .map_err(|_| AppError::internal("Failed to create CSRF cookie header"))?,
    );

    Ok((StatusCode::FOUND, resp_headers).into_response())
}

/// Resolve the `next` URL to redirect to, or `/libraries/` when it is unsafe.
///
/// Validation is by **origin**, not a path allowlist: the desktop client asks for
/// `/library/<repo-id>/<name>/` on "view on website" (`repo-tree-view.cpp:578`),
/// `/` from the account view and `/notification/list/` from the notification
/// monitor, and strips the scheme/host before sending. What matters for an
/// open-redirect is where the browser ends up:
///
/// * a relative path is accepted unless it is protocol-relative (`//evil.example`)
///   or escaped (`/\evil`, normalised to `//evil` by some browsers);
/// * an absolute URL only when its host matches this server's own origin.
pub(crate) fn resolve_next(next: Option<&str>, allowed_origin: &str) -> String {
    const FALLBACK: &str = "/libraries/";

    let Some(url) = next else {
        return FALLBACK.to_string();
    };
    if url.is_empty() {
        return FALLBACK.to_string();
    }
    // Strip surrounding whitespace and a NUL/CR/LF; header injection is already
    // impossible, but the value is compared below, so normalise it first.
    let url = url.trim();
    if url.chars().any(|c| c == '\0' || c == '\r' || c == '\n') {
        return FALLBACK.to_string();
    }

    if url.starts_with('/') {
        // `//host` is protocol-relative; `/\` is normalised to it by browsers.
        if url.starts_with("//") || url.starts_with("/\\") {
            return FALLBACK.to_string();
        }
        return url.to_string();
    }

    let Some((scheme, rest)) = url.split_once("://") else {
        return FALLBACK.to_string();
    };
    if !scheme.eq_ignore_ascii_case("http") && !scheme.eq_ignore_ascii_case("https") {
        return FALLBACK.to_string();
    }
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    if authority.is_empty() {
        return FALLBACK.to_string();
    }

    let allowed = allowed_origin.trim().trim_end_matches('/');
    let candidate = format!("{}://{}", scheme.to_ascii_lowercase(), authority);
    if candidate.eq_ignore_ascii_case(allowed) {
        url.to_string()
    } else {
        FALLBACK.to_string()
    }
}

#[cfg(test)]
mod resolve_next_tests {
    use super::resolve_next;

    const ORIGIN: &str = "https://files.example.com";

    #[test]
    fn relative_paths_are_kept() {
        // The desktop client asks for `/library/<id>/<name>/` on "view on website".
        for path in [
            "/library/abc/name/",
            "/libraries/",
            "/",
            "/notification/list/",
            "/shares/",
        ] {
            assert_eq!(resolve_next(Some(path), ORIGIN), path, "path {path}");
        }
    }

    #[test]
    fn protocol_relative_and_escaped_paths_are_rejected() {
        for hostile in ["//evil.example/x", "/\\evil.example", "/\\/evil.example"] {
            assert_eq!(
                resolve_next(Some(hostile), ORIGIN),
                "/libraries/",
                "{hostile} must not be honoured"
            );
        }
    }

    #[test]
    fn absolute_same_origin_urls_are_kept() {
        assert_eq!(
            resolve_next(Some("https://files.example.com/library/x/y/"), ORIGIN),
            "https://files.example.com/library/x/y/"
        );
    }

    #[test]
    fn other_origins_schemes_and_junk_fall_back() {
        for hostile in [
            "https://evil.example/",
            "https://files.example.com.evil.example/",
            "http://files.example.com/", // scheme downgrade is a different origin
            "javascript:alert(1)",
            "data:text/html,x",
            "",
            "   ",
        ] {
            assert_eq!(
                resolve_next(Some(hostile), ORIGIN),
                "/libraries/",
                "{hostile} must not be honoured"
            );
        }
        assert_eq!(resolve_next(None, ORIGIN), "/libraries/");
    }

    #[test]
    fn control_characters_are_rejected() {
        assert_eq!(
            resolve_next(Some("/library/x\r\nLocation: https://evil"), ORIGIN),
            "/libraries/"
        );
    }
}
