/// Web UI auth handlers — login page, login submission, TOTP verification, logout.
use askama::Template;
use axum::{
    Form,
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
    response::{Html, IntoResponse},
};
use serde::Deserialize;
use std::sync::Arc;

use crate::AppState;
use crate::i18n::I18n;
use crate::repository::api_token::CreateSessionTokenParams;
use crate::service::auth::password::verify_password_async;
use crate::service::auth::password_reset::PasswordResetService;
use crate::service::auth::registration::{RegistrationParams, RegistrationService};
use crate::service::auth::token::generate_api_token;
use crate::service::auth::totp::TotpManager;
use crate::ui::client_login::resolve_next;
use base::error::AppError;

// ─── Templates ───────────────────────────────────────────────────────────────

#[derive(Template)]
#[template(path = "page/login.html")]
pub struct LoginTemplate {
    /// Pre-computed static asset URLs with cache-busting hashes.
    pub urls: &'static crate::static_assets::TemplateUrls,
    pub t: &'static I18n,
    /// Error message to display (None = fresh form).
    pub error: Option<String>,
    /// Number of days the session will be remembered (shown in the UI).
    pub remember_days: u64,
    /// Whether password reset is enabled (show/hide "Forgot password?" link).
    pub enable_password_reset: bool,
    /// URL to redirect to after login (kept as a hidden form field).
    pub next: String,
}

/// Template shared with two_factor.rs for the TOTP entry page.
#[derive(Template)]
#[template(path = "page/two_factor_login.html")]
pub struct TwoFactorLoginTemplate {
    pub urls: &'static crate::static_assets::TemplateUrls,
    pub t: &'static I18n,
    pub error: Option<String>,
    /// URL to redirect to after 2FA verification.
    pub next: String,
}

// ─── Request types ───────────────────────────────────────────────────────────

/// `?next=` query parameter carried on the login pages (e.g. the SSO
/// local-browser bounce, which redirects to `/accounts/login/?next=...`).
#[derive(Deserialize)]
pub struct LoginNextQuery {
    pub next: Option<String>,
}

#[derive(Deserialize)]
pub struct LoginForm {
    pub email: String,
    pub password: String,
    /// "1" if the "Remember me" checkbox was checked.
    pub remember_me: Option<String>,
    /// Hidden form field echoing the original `?next=` query value.
    pub next: Option<String>,
}

#[derive(Deserialize)]
pub struct TwoFactorAuthForm {
    pub code: String,
    /// Hidden form field echoing the original `?next=` query value.
    pub next: Option<String>,
}

// ─── Handlers ────────────────────────────────────────────────────────────────

/// GET /accounts/login/ — render the login page.
pub async fn login_page(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(query): Query<LoginNextQuery>,
) -> Result<Html<String>, AppError> {
    let tpl = LoginTemplate {
        urls: crate::static_assets::template_urls(),
        t: I18n::from_headers(&headers, &state.config.ui.default_language),
        error: None,
        remember_days: state.config.auth.api_token_ttl_days,
        enable_password_reset: state.config.auth.enable_password_reset,
        next: query.next.unwrap_or_default(),
    };
    let html = tpl
        .render()
        .map_err(|e| AppError::internal(e.to_string()))?;
    Ok(Html(html))
}

/// Build a session cookie value with optional Secure flag.
fn session_cookie(name: &str, value: &str, max_age: Option<u64>, secure: bool) -> String {
    let mut cookie = format!("{}={}; HttpOnly; SameSite=Lax; Path=/", name, value);
    if let Some(age) = max_age {
        cookie.push_str(&format!("; Max-Age={}", age));
    }
    if secure {
        cookie.push_str("; Secure");
    }
    cookie
}

/// Create a short-lived pending token for the 2FA login flow.
async fn create_pending_token(
    repos: &crate::repository::Repositories,
    user_id: i32,
) -> Result<String, AppError> {
    let token = generate_api_token();
    let now = chrono::Utc::now().timestamp();
    // 5-minute TTL for the pending token
    let expires_at = now + 300;

    repos
        .api_token
        .create_session_token(CreateSessionTokenParams {
            user_id,
            token: token.clone(),
            created_at: now,
            expires_at: Some(expires_at),
            device_id: None,
            platform: None,
            device_name: None,
            client_version: None,
            is_pending: true,
        })
        .await
        .map_err(|e| AppError::internal(format!("failed to create pending token: {e}")))?;

    Ok(token)
}

async fn render_login_page(
    state: &Arc<AppState>,
    headers: &HeaderMap,
    error: Option<String>,
    next: &str,
) -> Result<Html<String>, AppError> {
    let tpl = LoginTemplate {
        urls: crate::static_assets::template_urls(),
        t: I18n::from_headers(headers, &state.config.ui.default_language),
        error,
        remember_days: state.config.auth.api_token_ttl_days,
        enable_password_reset: state.config.auth.enable_password_reset,
        next: next.to_string(),
    };
    let html = tpl
        .render()
        .map_err(|e| AppError::internal(e.to_string()))?;
    Ok(Html(html))
}

/// POST /accounts/login/ — authenticate user, handle 2FA if enabled.
pub async fn login(
    State(state): State<Arc<AppState>>,
    axum::extract::ConnectInfo(addr): axum::extract::ConnectInfo<std::net::SocketAddr>,
    headers: HeaderMap,
    Query(query): Query<LoginNextQuery>,
    Form(form): Form<LoginForm>,
) -> Result<impl IntoResponse, AppError> {
    // The hidden `next` form field carries the original `?next=` query value.
    // Fall back to the query itself for clients that post without the field.
    let next = resolve_next(form.next.as_deref().or(query.next.as_deref()));

    // CSRF: validate Origin/Referer.
    let origin = state.config.server.site_url_origin();
    if !crate::service::auth::csrf::validate_origin(&headers, &origin) {
        return render_login_page(
            &state,
            &headers,
            Some(
                I18n::from_headers(&headers, &state.config.ui.default_language)
                    .tr("auth.invalid_origin")
                    .to_string(),
            ),
            &next,
        )
        .await
        .map(|html| (StatusCode::FORBIDDEN, Html(html)).into_response());
    }

    // Client IP for rate limiting: TCP peer address (X-Forwarded-For is only
    // trusted from configured proxies), so spoofing the header can't bypass it.
    let client_ip = crate::middleware::effective_client_ip(
        &addr,
        &headers,
        &state.config.server.trusted_proxies,
    );

    // Composite keys: per-IP + per-username. No global key — a global lockout
    // would be a trivial denial-of-service against every user.
    let rate_limit_key_ip = format!("login:ip:{}", client_ip);
    let rate_limit_key_user = format!("login:user:{}", form.email);

    // Check rate limit before any DB or password work.
    if state
        .auth_limiters
        .login
        .is_any_locked(&[rate_limit_key_ip.as_str(), rate_limit_key_user.as_str()])
    {
        return render_login_page(
            &state,
            &headers,
            Some(
                I18n::from_headers(&headers, &state.config.ui.default_language)
                    .tr("auth.too_many_attempts")
                    .to_string(),
            ),
            &next,
        )
        .await
        .map(|html| (StatusCode::TOO_MANY_REQUESTS, Html(html)).into_response());
    }

    let user_record = match state.repos.user.find_by_email(&form.email).await? {
        Some(u) => u,
        None => {
            state
                .auth_limiters
                .login
                .record_failures(&[rate_limit_key_ip.as_str(), rate_limit_key_user.as_str()]);
            return render_login_page(
                &state,
                &headers,
                Some(
                    I18n::from_headers(&headers, &state.config.ui.default_language)
                        .tr("auth.incorrect_credentials")
                        .to_string(),
                ),
                &next,
            )
            .await
            .map(|html| (StatusCode::OK, Html(html)).into_response());
        }
    };

    if !user_record.is_active {
        // Use the same generic error to avoid user-enumeration attacks (matching seahub).
        state
            .auth_limiters
            .login
            .record_failures(&[rate_limit_key_ip.as_str(), rate_limit_key_user.as_str()]);
        return render_login_page(
            &state,
            &headers,
            Some(
                I18n::from_headers(&headers, &state.config.ui.default_language)
                    .tr("auth.incorrect_credentials")
                    .to_string(),
            ),
            &next,
        )
        .await
        .map(|html| (StatusCode::OK, Html(html)).into_response());
    }

    if !verify_password_async(
        form.password.clone(),
        user_record.password_hash.clone(),
        state.config.auth.password_hash_iterations,
    )
    .await
    {
        state
            .auth_limiters
            .login
            .record_failures(&[rate_limit_key_ip.as_str(), rate_limit_key_user.as_str()]);
        return render_login_page(
            &state,
            &headers,
            Some(
                I18n::from_headers(&headers, &state.config.ui.default_language)
                    .tr("auth.incorrect_credentials")
                    .to_string(),
            ),
            &next,
        )
        .await
        .map(|html| (StatusCode::OK, Html(html)).into_response());
    }

    // Successful login — clear rate limit for this IP and user.
    state.auth_limiters.login.clear(&rate_limit_key_ip);
    state.auth_limiters.login.clear(&rate_limit_key_user);

    // ── Check for 2FA ─────────────────────────────────────────────────
    let two_fa = state.repos.user_2fa.find_by_user_id(user_record.id).await?;

    if let Some(tfa) = two_fa
        && tfa.enabled
    {
        // User has 2FA enabled — create a pending token and redirect to TOTP page
        let pending_token = create_pending_token(&state.repos, user_record.id).await?;
        let cookie = session_cookie(
            "seahub-session-pending",
            &pending_token,
            Some(300),
            state.config.server.secure_cookies(),
        );

        // Thread the `next` URL through the 2FA step so the user still lands
        // on the original destination (e.g. the SSO confirm page) after TOTP.
        let encoded_next: String =
            percent_encoding::utf8_percent_encode(&next, percent_encoding::NON_ALPHANUMERIC)
                .collect();
        let two_fa_location = format!("/accounts/two-factor-auth/?next={encoded_next}");

        let response = (
            StatusCode::FOUND,
            [("Location", &two_fa_location), ("Set-Cookie", &cookie)],
        )
            .into_response();

        return Ok(response);
    }

    // ── No 2FA — normal login ───────────────────────────────────────
    let session_token = generate_api_token();
    let now = chrono::Utc::now().timestamp();

    // "Remember me" checkbox: checked → long-lived session; unchecked → session cookie.
    let is_remembered =
        form.remember_me.as_deref() == Some("1") || form.remember_me.as_deref() == Some("on");

    let ttl_days = state.config.auth.api_token_ttl_days;
    let token_expires_at = if is_remembered {
        Some(now + (ttl_days as i64 * 86400))
    } else {
        // Unchecked: short-lived token (24h).
        Some(now + 86400)
    };

    state
        .repos
        .api_token
        .create_session_token(CreateSessionTokenParams {
            user_id: user_record.id,
            token: session_token.clone(),
            created_at: now,
            expires_at: token_expires_at,
            device_id: None,
            platform: None,
            device_name: None,
            client_version: None,
            is_pending: false,
        })
        .await
        .map_err(|e| AppError::internal(format!("failed to create session token: {e}")))?;

    state
        .repos
        .user
        .touch_last_login(user_record.id, now)
        .await?;

    let secure_cookies = state.config.server.secure_cookies();
    let cookie = session_cookie(
        "seahub-session",
        &session_token,
        if is_remembered {
            Some(ttl_days * 86400)
        } else {
            Some(86400)
        },
        secure_cookies,
    );

    let csrf_max_age = if is_remembered {
        Some(ttl_days * 86400)
    } else {
        Some(86400)
    };
    let csrf_cookie = crate::service::auth::csrf::csrf_cookie_header(
        &state.csrf_secret,
        &session_token,
        secure_cookies,
        csrf_max_age,
    );

    let mut resp_headers = axum::http::HeaderMap::new();
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

    let response = (StatusCode::FOUND, resp_headers).into_response();

    Ok(response)
}

/// GET /accounts/two-factor-auth/ — show the TOTP verification page.
pub async fn two_factor_auth_page(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(query): Query<LoginNextQuery>,
) -> Result<Html<String>, AppError> {
    let tpl = TwoFactorLoginTemplate {
        urls: crate::static_assets::template_urls(),
        t: I18n::from_headers(&headers, &state.config.ui.default_language),
        error: None,
        next: query.next.unwrap_or_default(),
    };
    let html = tpl
        .render()
        .map_err(|e| AppError::internal(e.to_string()))?;
    Ok(Html(html))
}

/// POST /accounts/two-factor-auth/ — verify TOTP code and create session.
pub async fn two_factor_auth(
    State(state): State<Arc<AppState>>,
    axum::extract::ConnectInfo(addr): axum::extract::ConnectInfo<std::net::SocketAddr>,
    headers: HeaderMap,
    Query(query): Query<LoginNextQuery>,
    Form(form): Form<TwoFactorAuthForm>,
) -> Result<impl IntoResponse, AppError> {
    // The hidden `next` field carries the value from the pending 2FA redirect.
    let next = resolve_next(form.next.as_deref().or(query.next.as_deref()));

    // CSRF: validate Origin/Referer.
    let origin = state.config.server.site_url_origin();
    if !crate::service::auth::csrf::validate_origin(&headers, &origin) {
        return Err(AppError::BadRequest(
            I18n::from_headers(&headers, &state.config.ui.default_language)
                .tr("auth.invalid_origin")
                .to_string(),
        ));
    }

    // Client IP for rate limiting: TCP peer address (not spoofable XFF).
    let client_ip = crate::middleware::effective_client_ip(
        &addr,
        &headers,
        &state.config.server.trusted_proxies,
    );

    // Read the pending token from cookie
    let pending_token = headers
        .get("Cookie")
        .and_then(|v| v.to_str().ok())
        .and_then(|cookie_str| {
            cookie_str
                .split(';')
                .map(|s| s.trim())
                .find(|s| s.starts_with("seahub-session-pending="))
                .and_then(|s| s.strip_prefix("seahub-session-pending="))
        })
        .ok_or_else(|| {
            AppError::BadRequest(
                I18n::from_headers(&headers, &state.config.ui.default_language)
                    .tr("auth.session_expired")
                    .into(),
            )
        })?;

    // Look up the pending token
    let token_record = state
        .repos
        .api_token
        .find_by_token(pending_token)
        .await
        .map_err(|_| AppError::internal("database error"))?
        .ok_or_else(|| {
            AppError::BadRequest(
                I18n::from_headers(&headers, &state.config.ui.default_language)
                    .tr("auth.session_expired_alt")
                    .into(),
            )
        })?;

    // Check expiration
    if let Some(expires_at) = token_record.expires_at {
        let now = chrono::Utc::now().timestamp();
        if now > expires_at {
            let _ = state.repos.api_token.delete_by_token(pending_token).await;
            return Err(AppError::BadRequest(
                I18n::from_headers(&headers, &state.config.ui.default_language)
                    .tr("auth.session_expired")
                    .into(),
            ));
        }
    }

    let user_id = token_record.user_id;

    // Rate limit: per user+IP.
    let totp_key = format!("totp:{}:{}", user_id, client_ip);
    if state.auth_limiters.totp.is_limited(&totp_key) {
        // Delete pending token to force re-login
        let _ = state.repos.api_token.delete_by_token(pending_token).await;
        return Err(AppError::BadRequest(
            I18n::from_headers(&headers, &state.config.ui.default_language)
                .tr("auth.too_many_verification")
                .into(),
        ));
    }

    // Fetch user's 2FA config
    let two_fa = state
        .repos
        .user_2fa
        .find_by_user_id(user_id)
        .await?
        .ok_or_else(|| AppError::BadRequest("2FA is not configured for this account.".into()))?;

    if !two_fa.enabled {
        // 2FA was disabled since the pending token was created — proceed to login
        // Delete pending token, redirect to login page
        let _ = state.repos.api_token.delete_by_token(pending_token).await;
        return Ok((StatusCode::FOUND, [("Location", "/accounts/login/")]).into_response());
    }

    // Fetch user record for email
    let user_record = state
        .repos
        .user
        .find_by_id(user_id)
        .await?
        .ok_or(AppError::Unauthorized)?;

    // Verify the TOTP code
    let totp = TotpManager::create_totp(&two_fa.totp_secret, &user_record.email, "Nanofile")
        .map_err(|e| AppError::internal(e.to_string()))?;

    let code_valid = TotpManager::verify_code(&totp, &form.code);

    // Try backup code if TOTP failed
    let backup_valid = if !code_valid {
        crate::service::auth::backup_codes::BackupCodeManager::verify_code(
            &state.repos,
            user_id,
            &form.code,
        )
        .await
        .unwrap_or(false)
    } else {
        false
    };

    if !code_valid && !backup_valid {
        state.auth_limiters.totp.record_attempt(&totp_key);
        let tpl = TwoFactorLoginTemplate {
            urls: crate::static_assets::template_urls(),
            t: I18n::from_headers(&headers, &state.config.ui.default_language),
            error: Some(
                I18n::from_headers(&headers, &state.config.ui.default_language)
                    .tr("auth.invalid_code")
                    .to_string(),
            ),
            next: next.clone(),
        };
        let html = tpl
            .render()
            .map_err(|e| AppError::internal(e.to_string()))?;
        return Ok((StatusCode::OK, Html(html)).into_response());
    }

    // ── TOTP verified — create real session ────────────────────────
    state.auth_limiters.totp.clear(&totp_key);
    // Delete the pending token
    let _ = state.repos.api_token.delete_by_token(pending_token).await;

    let session_token = generate_api_token();
    let now = chrono::Utc::now().timestamp();
    let ttl_days = state.config.auth.api_token_ttl_days;
    let expires_at = now + (ttl_days as i64 * 86400);

    state
        .repos
        .api_token
        .create_session_token(CreateSessionTokenParams {
            user_id,
            token: session_token.clone(),
            created_at: now,
            expires_at: Some(expires_at),
            device_id: None,
            platform: None,
            device_name: None,
            client_version: None,
            is_pending: false,
        })
        .await
        .map_err(|e| AppError::internal(format!("failed to create session token: {e}")))?;

    state.repos.user.touch_last_login(user_id, now).await?;

    let secure_cookies = state.config.server.secure_cookies();
    let session_cookie_str = session_cookie(
        "seahub-session",
        &session_token,
        Some(ttl_days * 86400),
        secure_cookies,
    );

    let clear_pending_str = session_cookie("seahub-session-pending", "", Some(0), secure_cookies);

    let csrf_cookie_str = crate::service::auth::csrf::csrf_cookie_header(
        &state.csrf_secret,
        &session_token,
        secure_cookies,
        Some(ttl_days * 86400),
    );

    // HeaderMap::append so all Set-Cookie headers reach the browser.
    // A plain array tuple uses `insert` which would overwrite the first one.
    let mut resp_headers = ::axum::http::HeaderMap::new();
    resp_headers.insert(
        ::axum::http::header::LOCATION,
        ::axum::http::HeaderValue::from_str(&next)
            .map_err(|_| AppError::internal("Failed to create location header"))?,
    );
    resp_headers.append(
        ::axum::http::header::SET_COOKIE,
        session_cookie_str
            .parse::<::axum::http::HeaderValue>()
            .map_err(|_| AppError::internal("Failed to create session cookie header"))?,
    );
    resp_headers.append(
        ::axum::http::header::SET_COOKIE,
        clear_pending_str
            .parse::<::axum::http::HeaderValue>()
            .map_err(|_| AppError::internal("Failed to create session cookie header"))?,
    );
    resp_headers.append(
        ::axum::http::header::SET_COOKIE,
        csrf_cookie_str
            .parse::<::axum::http::HeaderValue>()
            .map_err(|_| AppError::internal("Failed to create CSRF cookie header"))?,
    );

    Ok((StatusCode::FOUND, resp_headers).into_response())
}

/// GET /accounts/logout/ — log out (clear session cookie, delete token).
pub async fn logout(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Result<impl IntoResponse, AppError> {
    if let Some(cookie_str) = headers.get("Cookie").and_then(|v| v.to_str().ok())
        && let Some(token) = cookie_str
            .split(';')
            .map(|s| s.trim())
            .find(|s| s.starts_with("seahub-session="))
            .and_then(|s| s.strip_prefix("seahub-session="))
    {
        let _ = state.repos.api_token.delete_by_token(token).await;
    }

    let secure_cookies = state.config.server.secure_cookies();
    let clear_cookie = session_cookie("seahub-session", "", Some(0), secure_cookies);
    // Clear the CSRF token cookie too.
    let mut clear_csrf = String::from("sfcsrftoken=; Path=/; SameSite=Lax; Max-Age=0");
    if secure_cookies {
        clear_csrf.push_str("; Secure");
    }

    let mut resp_headers = axum::http::HeaderMap::new();
    resp_headers.insert(
        axum::http::header::LOCATION,
        axum::http::HeaderValue::from_static("/accounts/login/"),
    );
    resp_headers.append(
        axum::http::header::SET_COOKIE,
        clear_cookie
            .parse::<axum::http::HeaderValue>()
            .map_err(|_| AppError::internal("Failed to create session cookie header"))?,
    );
    resp_headers.append(
        axum::http::header::SET_COOKIE,
        clear_csrf
            .parse::<axum::http::HeaderValue>()
            .map_err(|_| AppError::internal("Failed to create CSRF cookie header"))?,
    );

    let response = (StatusCode::FOUND, resp_headers).into_response();

    Ok(response)
}

// ─── Registration (invitation-only) ─────────────────────────────────────────

#[derive(Template)]
#[template(path = "page/register.html")]
pub struct RegisterTemplate {
    pub urls: &'static crate::static_assets::TemplateUrls,
    pub t: &'static I18n,
    pub error: Option<String>,
}

#[derive(Deserialize)]
pub struct RegisterForm {
    pub email: String,
    pub password1: String,
    pub password2: String,
    pub invitation_code: String,
}

/// GET /accounts/register/ — render the registration form.
pub async fn register_page(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Result<Html<String>, AppError> {
    let tpl = RegisterTemplate {
        urls: crate::static_assets::template_urls(),
        t: I18n::from_headers(&headers, &state.config.ui.default_language),
        error: None,
    };
    let html = tpl
        .render()
        .map_err(|e| AppError::internal(e.to_string()))?;
    Ok(Html(html))
}

/// POST /accounts/register/ — validate invitation code and create account.
pub async fn register(
    State(state): State<Arc<AppState>>,
    axum::extract::ConnectInfo(addr): axum::extract::ConnectInfo<std::net::SocketAddr>,
    headers: HeaderMap,
    Form(form): Form<RegisterForm>,
) -> Result<impl IntoResponse, AppError> {
    // CSRF: validate Origin/Referer.
    let origin = state.config.server.site_url_origin();
    if !crate::service::auth::csrf::validate_origin(&headers, &origin) {
        return Err(AppError::BadRequest(
            I18n::from_headers(&headers, &state.config.ui.default_language)
                .tr("auth.invalid_origin")
                .to_string(),
        ));
    }

    // Rate limit: per IP (TCP peer, not spoofable XFF).
    let client_ip = crate::middleware::effective_client_ip(
        &addr,
        &headers,
        &state.config.server.trusted_proxies,
    );
    let rl_key = format!("register:{}", client_ip);
    if state.auth_limiters.registration.is_limited(&rl_key) {
        return Err(AppError::BadRequest(
            I18n::from_headers(&headers, &state.config.ui.default_language)
                .tr("auth.registration_too_many")
                .to_string(),
        ));
    }
    state.auth_limiters.registration.record_attempt(&rl_key);

    // Validate passwords match before delegating to service.
    if form.password1 != form.password2 {
        return Err(AppError::BadRequest(
            I18n::from_headers(&headers, &state.config.ui.default_language)
                .tr("auth.passwords_mismatch")
                .to_string(),
        ));
    }

    let cfg = &state.config.auth;

    // Use RegistrationService for the core logic.
    let reg_service = RegistrationService::new(state.repos.clone());
    let result = reg_service
        .register(RegistrationParams {
            email: form.email,
            password: form.password1,
            invitation_code: form.invitation_code,
            password_min_length: cfg.password_min_length as usize,
            require_strong_password: cfg.require_strong_password,
            password_hash_iterations: cfg.password_hash_iterations,
        })
        .await?;

    // Auto-login — create session token and cookie.
    let now = chrono::Utc::now().timestamp();
    let session_token = generate_api_token();
    let ttl_days = cfg.api_token_ttl_days;

    state
        .repos
        .api_token
        .create_session_token(CreateSessionTokenParams {
            user_id: result.user.id,
            token: session_token.clone(),
            created_at: now,
            expires_at: Some(now + (ttl_days as i64 * 86400)),
            device_id: None,
            platform: None,
            device_name: None,
            client_version: None,
            is_pending: false,
        })
        .await
        .map_err(|e| AppError::internal(format!("failed to create session token: {e}")))?;

    state
        .repos
        .user
        .touch_last_login(result.user.id, now)
        .await?;

    let secure_cookies = state.config.server.secure_cookies();
    let cookie = session_cookie(
        "seahub-session",
        &session_token,
        Some(ttl_days * 86400),
        secure_cookies,
    );

    let csrf_cookie = crate::service::auth::csrf::csrf_cookie_header(
        &state.csrf_secret,
        &session_token,
        secure_cookies,
        Some(ttl_days * 86400),
    );

    let mut resp_headers = axum::http::HeaderMap::new();
    resp_headers.insert(
        axum::http::header::LOCATION,
        axum::http::HeaderValue::from_static("/libraries/"),
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

// ─── Password Reset ─────────────────────────────────────────────────────────

#[derive(Template)]
#[template(path = "page/password_reset_form.html")]
pub struct PasswordResetFormTemplate {
    pub urls: &'static crate::static_assets::TemplateUrls,
    pub t: &'static I18n,
    pub error: Option<String>,
}

#[derive(Template)]
#[template(path = "page/password_reset_done.html")]
pub struct PasswordResetDoneTemplate {
    pub urls: &'static crate::static_assets::TemplateUrls,
    pub t: &'static I18n,
}

#[derive(Template)]
#[template(path = "page/password_reset_confirm.html")]
pub struct PasswordResetConfirmTemplate {
    pub urls: &'static crate::static_assets::TemplateUrls,
    pub t: &'static I18n,
    pub error: Option<String>,
    /// Whether the token is valid (show form) or invalid (show error).
    pub valid: bool,
}

#[derive(Template)]
#[template(path = "page/password_reset_complete.html")]
pub struct PasswordResetCompleteTemplate {
    pub urls: &'static crate::static_assets::TemplateUrls,
    pub t: &'static I18n,
}

#[derive(Deserialize)]
pub struct PasswordResetForm {
    pub email: String,
}

#[derive(Deserialize)]
pub struct PasswordResetConfirmForm {
    pub password1: String,
    pub password2: String,
}

/// GET /accounts/password/reset/ — show the password reset request form.
pub async fn password_reset_page(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Result<Html<String>, AppError> {
    let tpl = PasswordResetFormTemplate {
        urls: crate::static_assets::template_urls(),
        t: I18n::from_headers(&headers, &state.config.ui.default_language),
        error: None,
    };
    let html = tpl
        .render()
        .map_err(|e| AppError::internal(e.to_string()))?;
    Ok(Html(html))
}

/// POST /accounts/password/reset/ — request a password reset.
///
/// Security: the reset link is **never** returned in the HTTP response. Without
/// a configured email backend the flow is disabled entirely (no token is
/// minted), so the response can never be used to take over an account.
pub async fn password_reset(
    State(state): State<Arc<AppState>>,
    axum::extract::ConnectInfo(addr): axum::extract::ConnectInfo<std::net::SocketAddr>,
    headers: HeaderMap,
    Form(form): Form<PasswordResetForm>,
) -> Result<Html<String>, AppError> {
    // CSRF: validate Origin/Referer.
    let origin = state.config.server.site_url_origin();
    if !crate::service::auth::csrf::validate_origin(&headers, &origin) {
        return Ok(Html(String::new()));
    }

    // Email delivery is required to hand the reset link to the account owner.
    // Without it, minting a token and echoing the link back would let anyone
    // reset any account. Render the generic page and do not create a token.
    if !state.config.email.enabled {
        let tpl = PasswordResetDoneTemplate {
            urls: crate::static_assets::template_urls(),
            t: I18n::from_headers(&headers, &state.config.ui.default_language),
        };
        let html = tpl
            .render()
            .map_err(|e| AppError::internal(e.to_string()))?;
        return Ok(Html(html));
    }

    // Rate limit: per IP (TCP peer, not spoofable XFF).
    let client_ip = crate::middleware::effective_client_ip(
        &addr,
        &headers,
        &state.config.server.trusted_proxies,
    );
    let rl_key = format!("password_reset:{}", client_ip);
    if state.auth_limiters.password_reset.is_limited(&rl_key) {
        // Show the done page silently to prevent enumeration.
        let tpl = PasswordResetDoneTemplate {
            urls: crate::static_assets::template_urls(),
            t: I18n::from_headers(&headers, &state.config.ui.default_language),
        };
        let html = tpl
            .render()
            .map_err(|e| AppError::internal(e.to_string()))?;
        return Ok(Html(html));
    }
    state.auth_limiters.password_reset.record_attempt(&rl_key);

    // Mint a token that will be emailed to the account owner (delivery backend
    // not yet wired up). The response deliberately does not contain it.
    let reset_service = PasswordResetService::new(state.repos.clone());
    let _result = reset_service
        .create_reset_token(&form.email, &state.config.server.site_url)
        .await?;

    let tpl = PasswordResetDoneTemplate {
        urls: crate::static_assets::template_urls(),
        t: I18n::from_headers(&headers, &state.config.ui.default_language),
    };
    let html = tpl
        .render()
        .map_err(|e| AppError::internal(e.to_string()))?;
    Ok(Html(html))
}

/// GET /accounts/password/reset/done/ — show confirmation (accessed directly, no URL).
pub async fn password_reset_done(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Result<Html<String>, AppError> {
    let tpl = PasswordResetDoneTemplate {
        urls: crate::static_assets::template_urls(),
        t: I18n::from_headers(&headers, &state.config.ui.default_language),
    };
    let html = tpl
        .render()
        .map_err(|e| AppError::internal(e.to_string()))?;
    Ok(Html(html))
}

/// GET /accounts/password/reset/{token}/ — show the new password form or error.
pub async fn password_reset_confirm_page(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(token): Path<String>,
) -> Result<Html<String>, AppError> {
    let reset_service = PasswordResetService::new(state.repos.clone());
    let valid = reset_service.validate_token(&token).await?.is_some();

    let tpl = PasswordResetConfirmTemplate {
        urls: crate::static_assets::template_urls(),
        t: I18n::from_headers(&headers, &state.config.ui.default_language),
        error: None,
        valid,
    };
    let html = tpl
        .render()
        .map_err(|e| AppError::internal(e.to_string()))?;
    Ok(Html(html))
}

/// POST /accounts/password/reset/{token}/ — validate token and update password.
pub async fn password_reset_confirm(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(token): Path<String>,
    Form(form): Form<PasswordResetConfirmForm>,
) -> Result<impl IntoResponse, AppError> {
    // CSRF: validate Origin/Referer.
    let origin = state.config.server.site_url_origin();
    if !crate::service::auth::csrf::validate_origin(&headers, &origin) {
        return Ok(StatusCode::FORBIDDEN.into_response());
    }

    let cfg = &state.config.auth;

    // Validate passwords match before delegating to service.
    if form.password1 != form.password2 {
        let tpl = PasswordResetConfirmTemplate {
            urls: crate::static_assets::template_urls(),
            t: I18n::from_headers(&headers, &state.config.ui.default_language),
            error: Some(
                I18n::from_headers(&headers, &state.config.ui.default_language)
                    .tr("auth.passwords_mismatch")
                    .to_string(),
            ),
            valid: true,
        };
        let html = tpl
            .render()
            .map_err(|e| AppError::internal(e.to_string()))?;
        return Ok((StatusCode::OK, Html(html)).into_response());
    }

    // Use PasswordResetService.
    let reset_service = PasswordResetService::new(state.repos.clone());
    match reset_service
        .reset_password(
            &token,
            &form.password1,
            cfg.password_min_length as usize,
            cfg.require_strong_password,
            cfg.password_hash_iterations,
        )
        .await
    {
        Ok(()) => {}
        Err(AppError::BadRequest(msg)) => {
            let tpl = PasswordResetConfirmTemplate {
                urls: crate::static_assets::template_urls(),
                t: I18n::from_headers(&headers, &state.config.ui.default_language),
                error: Some(msg),
                valid: true,
            };
            let html = tpl
                .render()
                .map_err(|e| AppError::internal(e.to_string()))?;
            return Ok((StatusCode::OK, Html(html)).into_response());
        }
        Err(e) => return Err(e),
    }

    Ok((
        StatusCode::FOUND,
        [("Location", "/accounts/password/reset/complete/")],
    )
        .into_response())
}

/// GET /accounts/password/reset/complete/ — show success page.
pub async fn password_reset_complete(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Result<Html<String>, AppError> {
    let tpl = PasswordResetCompleteTemplate {
        urls: crate::static_assets::template_urls(),
        t: I18n::from_headers(&headers, &state.config.ui.default_language),
    };
    let html = tpl
        .render()
        .map_err(|e| AppError::internal(e.to_string()))?;
    Ok(Html(html))
}
