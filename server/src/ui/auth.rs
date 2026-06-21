/// Web UI auth handlers — login page, login submission, TOTP verification, logout.
use askama::Template;
use axum::{
    Form,
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    response::{Html, IntoResponse},
};
use sea_orm::{ActiveModelTrait, ColumnTrait, EntityTrait, ModelTrait, QueryFilter, Set};
use serde::Deserialize;
use std::sync::Arc;

use crate::AppState;
use crate::auth::password::{hash_password, validate_password, verify_password};
use crate::auth::token::generate_api_token;
use crate::auth::totp::TotpManager;
use crate::entity::{api_token, invitation_code, user, user_2fa};
use crate::error::AppError;

// ─── Templates ───────────────────────────────────────────────────────────────

#[derive(Template)]
#[template(path = "page/login.html")]
pub struct LoginTemplate {
    /// Pre-computed static asset URLs with cache-busting hashes.
    pub urls: &'static crate::static_assets::TemplateUrls,
    /// Error message to display (None = fresh form).
    pub error: Option<String>,
    /// Number of days the session will be remembered (shown in the UI).
    pub remember_days: u64,
    /// Whether password reset is enabled (show/hide "Forgot password?" link).
    pub enable_password_reset: bool,
}

/// Template shared with two_factor.rs for the TOTP entry page.
#[derive(Template)]
#[template(path = "page/two_factor_login.html")]
pub struct TwoFactorLoginTemplate {
    pub urls: &'static crate::static_assets::TemplateUrls,
    pub error: Option<String>,
}

// ─── Request types ───────────────────────────────────────────────────────────

#[derive(Deserialize)]
pub struct LoginForm {
    pub email: String,
    pub password: String,
    /// "1" if the "Remember me" checkbox was checked.
    pub remember_me: Option<String>,
}

#[derive(Deserialize)]
pub struct TwoFactorAuthForm {
    pub code: String,
}

// ─── Handlers ────────────────────────────────────────────────────────────────

/// GET /accounts/login/ — render the login page.
pub async fn login_page(State(state): State<Arc<AppState>>) -> Result<Html<String>, AppError> {
    let tpl = LoginTemplate {
        urls: crate::static_assets::template_urls(),
        error: None,
        remember_days: state.config.auth.api_token_ttl_days,
        enable_password_reset: state.config.auth.enable_password_reset,
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
    db: &sea_orm::DatabaseConnection,
    user_id: i32,
) -> Result<String, AppError> {
    let token = generate_api_token();
    let now = chrono::Utc::now().timestamp();
    // 5-minute TTL for the pending token
    let expires_at = now + 300;

    let token_record = api_token::ActiveModel {
        id: sea_orm::NotSet,
        user_id: Set(user_id),
        token: Set(token.clone()),
        created_at: Set(now),
        expires_at: Set(Some(expires_at)),
        device_id: Set(None),
        platform: Set(None),
        device_name: Set(None),
        client_version: Set(None),
    };

    token_record
        .insert(db)
        .await
        .map_err(|e| AppError::internal(format!("failed to create pending token: {e}")))?;

    Ok(token)
}

async fn render_login_page(
    state: &Arc<AppState>,
    error: Option<String>,
) -> Result<Html<String>, AppError> {
    let tpl = LoginTemplate {
        urls: crate::static_assets::template_urls(),
        error,
        remember_days: state.config.auth.api_token_ttl_days,
        enable_password_reset: state.config.auth.enable_password_reset,
    };
    let html = tpl
        .render()
        .map_err(|e| AppError::internal(e.to_string()))?;
    Ok(Html(html))
}

/// POST /accounts/login/ — authenticate user, handle 2FA if enabled.
pub async fn login(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Form(form): Form<LoginForm>,
) -> Result<impl IntoResponse, AppError> {
    // CSRF: validate Origin/Referer.
    let origin = state.config.server.site_url_origin();
    if !crate::auth::csrf::validate_origin(&headers, &origin) {
        return render_login_page(&state, Some("Invalid request origin.".to_string()))
            .await
            .map(|html| (StatusCode::FORBIDDEN, Html(html)).into_response());
    }

    let db = state.db.as_ref();

    // Extract client IP for rate limiting.
    let client_ip = headers
        .get("x-forwarded-for")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.split(',').next().map(|s| s.trim().to_string()))
        .or_else(|| {
            headers
                .get("x-real-ip")
                .and_then(|v| v.to_str().ok())
                .map(|s| s.to_string())
        })
        .unwrap_or_else(|| "unknown".to_string());

    // Use composite keys: per-IP + per-username + global, so that rotating
    // spoofed X-Forwarded-For headers cannot bypass the rate limiter.
    let rate_limit_key_ip = format!("login:ip:{}", client_ip);
    let rate_limit_key_user = format!("login:user:{}", form.email);
    let rate_limit_key_global = "login:global".to_string();

    // Check rate limit before any DB or password work.
    if state.login_rate_limiter.is_locked(&rate_limit_key_ip)
        || state.login_rate_limiter.is_locked(&rate_limit_key_user)
        || state.login_rate_limiter.is_locked(&rate_limit_key_global)
    {
        return render_login_page(
            &state,
            Some("Too many login attempts. Please try again later.".to_string()),
        )
        .await
        .map(|html| (StatusCode::TOO_MANY_REQUESTS, Html(html)).into_response());
    }

    let user_record = user::Entity::find()
        .filter(user::Column::Email.eq(&form.email))
        .one(db)
        .await
        .map_err(|_| AppError::internal("database error"))?;

    let user_record = match user_record {
        Some(u) => u,
        None => {
            state.login_rate_limiter.record_failure(&rate_limit_key_ip);
            state
                .login_rate_limiter
                .record_failure(&rate_limit_key_user);
            state
                .login_rate_limiter
                .record_failure(&rate_limit_key_global);
            return render_login_page(&state, Some("Incorrect email or password.".to_string()))
                .await
                .map(|html| (StatusCode::OK, Html(html)).into_response());
        }
    };

    if !user_record.is_active {
        // Use the same generic error to avoid user-enumeration attacks (matching seahub).
        state.login_rate_limiter.record_failure(&rate_limit_key_ip);
        state
            .login_rate_limiter
            .record_failure(&rate_limit_key_user);
        state
            .login_rate_limiter
            .record_failure(&rate_limit_key_global);
        return render_login_page(&state, Some("Incorrect email or password.".to_string()))
            .await
            .map(|html| (StatusCode::OK, Html(html)).into_response());
    }

    if !verify_password(
        &form.password,
        &user_record.password_hash,
        state.config.auth.password_hash_iterations,
    ) {
        state.login_rate_limiter.record_failure(&rate_limit_key_ip);
        state
            .login_rate_limiter
            .record_failure(&rate_limit_key_user);
        state
            .login_rate_limiter
            .record_failure(&rate_limit_key_global);
        return render_login_page(&state, Some("Incorrect email or password.".to_string()))
            .await
            .map(|html| (StatusCode::OK, Html(html)).into_response());
    }

    // Successful login — clear rate limit for this IP.
    state.login_rate_limiter.clear(&rate_limit_key_ip);
    state.login_rate_limiter.clear(&rate_limit_key_user);
    state.login_rate_limiter.clear(&rate_limit_key_global);

    // ── Check for 2FA ─────────────────────────────────────────────────
    let two_fa = user_2fa::Entity::find_by_id(user_record.id).one(db).await?;

    if let Some(tfa) = two_fa
        && tfa.enabled
    {
        // User has 2FA enabled — create a pending token and redirect to TOTP page
        let pending_token = create_pending_token(db, user_record.id).await?;
        let cookie = session_cookie(
            "seahub-session-pending",
            &pending_token,
            Some(300),
            state.config.server.secure_cookies(),
        );

        let response = (
            StatusCode::FOUND,
            [
                ("Location", "/accounts/two-factor-auth/"),
                ("Set-Cookie", &cookie),
            ],
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

    let token_record = api_token::ActiveModel {
        id: sea_orm::NotSet,
        user_id: Set(user_record.id),
        token: Set(session_token.clone()),
        created_at: Set(now),
        expires_at: Set(token_expires_at),
        device_id: Set(None),
        platform: Set(None),
        device_name: Set(None),
        client_version: Set(None),
    };

    token_record
        .insert(db)
        .await
        .map_err(|e| AppError::internal(format!("failed to create session token: {e}")))?;

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

    let csrf_cookie =
        crate::auth::csrf::csrf_cookie_header(&state.csrf_secret, &session_token, secure_cookies);

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

    let response = (StatusCode::FOUND, resp_headers).into_response();

    Ok(response)
}

/// GET /accounts/two-factor-auth/ — show the TOTP verification page.
pub async fn two_factor_auth_page() -> Result<Html<String>, AppError> {
    let tpl = TwoFactorLoginTemplate {
        urls: crate::static_assets::template_urls(),
        error: None,
    };
    let html = tpl
        .render()
        .map_err(|e| AppError::internal(e.to_string()))?;
    Ok(Html(html))
}

/// POST /accounts/two-factor-auth/ — verify TOTP code and create session.
pub async fn two_factor_auth(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Form(form): Form<TwoFactorAuthForm>,
) -> Result<impl IntoResponse, AppError> {
    // CSRF: validate Origin/Referer.
    let origin = state.config.server.site_url_origin();
    if !crate::auth::csrf::validate_origin(&headers, &origin) {
        return Err(AppError::BadRequest("Invalid request origin.".to_string()));
    }
    let db = state.db.as_ref();

    // Extract client IP for rate limiting.
    let client_ip = headers
        .get("x-forwarded-for")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.split(',').next().map(|s| s.trim().to_string()))
        .or_else(|| {
            headers
                .get("x-real-ip")
                .and_then(|v| v.to_str().ok())
                .map(|s| s.to_string())
        })
        .unwrap_or_else(|| "unknown".to_string());

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
            AppError::BadRequest("Authentication session expired. Please log in again.".into())
        })?;

    // Look up the pending token
    let token_record = api_token::Entity::find()
        .filter(api_token::Column::Token.eq(pending_token))
        .one(db)
        .await
        .map_err(|_| AppError::internal("database error"))?
        .ok_or_else(|| {
            AppError::BadRequest(
                "Invalid or expired authentication session. Please log in again.".into(),
            )
        })?;

    // Check expiration
    if let Some(expires_at) = token_record.expires_at {
        let now = chrono::Utc::now().timestamp();
        if now > expires_at {
            let _ = token_record.delete(db).await;
            return Err(AppError::BadRequest(
                "Authentication session expired. Please log in again.".into(),
            ));
        }
    }

    let user_id = token_record.user_id;

    // Rate limit: per user+IP.
    let totp_key = format!("totp:{}:{}", user_id, client_ip);
    if state.totp_limiter.is_limited(&totp_key) {
        // Delete pending token to force re-login
        let _ = token_record.delete(db).await;
        return Err(AppError::BadRequest(
            "Too many verification attempts. Please log in again.".into(),
        ));
    }

    // Fetch user's 2FA config
    let two_fa = user_2fa::Entity::find_by_id(user_id)
        .one(db)
        .await?
        .ok_or_else(|| AppError::BadRequest("2FA is not configured for this account.".into()))?;

    if !two_fa.enabled {
        // 2FA was disabled since the pending token was created — proceed to login
        // Delete pending token, redirect to login page
        let _ = token_record.delete(db).await;
        return Ok((StatusCode::FOUND, [("Location", "/accounts/login/")]).into_response());
    }

    // Fetch user record for email
    let user_record = user::Entity::find_by_id(user_id)
        .one(db)
        .await?
        .ok_or(AppError::Unauthorized)?;

    // Verify the TOTP code
    let totp = TotpManager::create_totp(&two_fa.totp_secret, &user_record.email, "Nanofile")
        .map_err(|e| AppError::internal(e.to_string()))?;

    let code_valid = TotpManager::verify_code(&totp, &form.code);

    // Try backup code if TOTP failed
    let backup_valid = if !code_valid {
        crate::auth::backup_codes::BackupCodeManager::verify_code(db, user_id, &form.code)
            .await
            .unwrap_or(false)
    } else {
        false
    };

    if !code_valid && !backup_valid {
        state.totp_limiter.record_attempt(&totp_key);
        let tpl = TwoFactorLoginTemplate {
            urls: crate::static_assets::template_urls(),
            error: Some("Invalid verification code. Please try again.".to_string()),
        };
        let html = tpl
            .render()
            .map_err(|e| AppError::internal(e.to_string()))?;
        return Ok((StatusCode::OK, Html(html)).into_response());
    }

    // ── TOTP verified — create real session ────────────────────────
    state.totp_limiter.clear(&totp_key);
    // Delete the pending token
    let _ = token_record.delete(db).await;

    let session_token = generate_api_token();
    let now = chrono::Utc::now().timestamp();
    let ttl_days = state.config.auth.api_token_ttl_days;
    let expires_at = now + (ttl_days as i64 * 86400);

    let new_token = api_token::ActiveModel {
        id: sea_orm::NotSet,
        user_id: Set(user_id),
        token: Set(session_token.clone()),
        created_at: Set(now),
        expires_at: Set(Some(expires_at)),
        device_id: Set(None),
        platform: Set(None),
        device_name: Set(None),
        client_version: Set(None),
    };

    new_token
        .insert(db)
        .await
        .map_err(|e| AppError::internal(format!("failed to create session token: {e}")))?;

    let secure_cookies = state.config.server.secure_cookies();
    let session_cookie_str = session_cookie(
        "seahub-session",
        &session_token,
        Some(ttl_days * 86400),
        secure_cookies,
    );

    let clear_pending_str = session_cookie("seahub-session-pending", "", Some(0), secure_cookies);

    let csrf_cookie_str =
        crate::auth::csrf::csrf_cookie_header(&state.csrf_secret, &session_token, secure_cookies);

    // HeaderMap::append so all Set-Cookie headers reach the browser.
    // A plain array tuple uses `insert` which would overwrite the first one.
    let mut resp_headers = ::axum::http::HeaderMap::new();
    resp_headers.insert(
        ::axum::http::header::LOCATION,
        ::axum::http::HeaderValue::from_static("/libraries/"),
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
        let _ = api_token::Entity::delete_many()
            .filter(api_token::Column::Token.eq(token))
            .exec(state.db.as_ref())
            .await;
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
pub async fn register_page() -> Result<Html<String>, AppError> {
    let tpl = RegisterTemplate {
        urls: crate::static_assets::template_urls(),
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
    headers: HeaderMap,
    Form(form): Form<RegisterForm>,
) -> Result<impl IntoResponse, AppError> {
    // CSRF: validate Origin/Referer.
    let origin = state.config.server.site_url_origin();
    if !crate::auth::csrf::validate_origin(&headers, &origin) {
        return Err(AppError::BadRequest("Invalid request origin.".to_string()));
    }

    // Rate limit: per IP.
    let client_ip = headers
        .get("x-forwarded-for")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.split(',').next().map(|s| s.trim().to_string()))
        .or_else(|| {
            headers
                .get("x-real-ip")
                .and_then(|v| v.to_str().ok())
                .map(|s| s.to_string())
        })
        .unwrap_or_else(|| "unknown".to_string());
    let rl_key = format!("register:{}", client_ip);
    if state.registration_limiter.is_limited(&rl_key) {
        return Err(AppError::BadRequest(
            "Too many registration attempts. Try again later.".to_string(),
        ));
    }
    state.registration_limiter.record_attempt(&rl_key);

    let db = state.db.as_ref();

    // 1. Validate invitation code.
    let code_record = invitation_code::Entity::find()
        .filter(invitation_code::Column::Code.eq(&form.invitation_code))
        .one(db)
        .await?
        .ok_or_else(|| AppError::BadRequest("Invalid invitation code.".to_string()))?;

    if code_record.used_by.is_some() {
        return Err(AppError::BadRequest(
            "This invitation code has already been used.".to_string(),
        ));
    }

    // 2. Check email binding (if the code is bound to a specific email).
    if let Some(ref bound_email) = code_record.email
        && bound_email.to_lowercase() != form.email.to_lowercase()
    {
        return Err(AppError::BadRequest(
            "This invitation code is bound to a different email address.".to_string(),
        ));
    }

    // 3. Validate email uniqueness.
    let existing = user::Entity::find()
        .filter(user::Column::Email.eq(&form.email))
        .one(db)
        .await?;
    if existing.is_some() {
        return Err(AppError::BadRequest(
            "A user with this email already exists.".to_string(),
        ));
    }

    // 4. Validate passwords match.
    if form.password1 != form.password2 {
        return Err(AppError::BadRequest("Passwords do not match.".to_string()));
    }

    // 5. Validate password strength.
    let cfg = &state.config.auth;
    if let Err(msg) = validate_password(
        &form.password1,
        cfg.password_min_length,
        cfg.require_strong_password,
    ) {
        return Err(AppError::BadRequest(msg));
    }

    // 6. Create the user.
    let now = chrono::Utc::now().timestamp();
    let iterations = state.config.auth.password_hash_iterations;
    let password_hash = hash_password(&form.password1, iterations);

    let new_user = user::ActiveModel {
        id: sea_orm::NotSet,
        email: Set(form.email),
        password_hash: Set(password_hash),
        is_active: Set(true),
        is_admin: Set(false),
        created_at: Set(now),
        last_login_at: Set(None),
        invited_by: Set(Some(code_record.creator_id)),
        name: sea_orm::NotSet,
        display_name: sea_orm::NotSet,
    };

    let new_user = new_user
        .insert(db)
        .await
        .map_err(|e| AppError::internal(format!("failed to create user: {e}")))?;

    // 7. Mark invitation code as used.
    let mut active_code: invitation_code::ActiveModel = code_record.into();
    active_code.used_by = Set(Some(new_user.id));
    active_code.used_at = Set(Some(now));
    active_code.update(db).await?;

    // 8. Auto-login — create session token and cookie.
    let session_token = generate_api_token();
    let ttl_days = cfg.api_token_ttl_days;

    let token_record = api_token::ActiveModel {
        id: sea_orm::NotSet,
        user_id: Set(new_user.id),
        token: Set(session_token.clone()),
        created_at: Set(now),
        expires_at: Set(Some(now + (ttl_days as i64 * 86400))),
        device_id: Set(None),
        platform: Set(None),
        device_name: Set(None),
        client_version: Set(None),
    };

    token_record
        .insert(db)
        .await
        .map_err(|e| AppError::internal(format!("failed to create session token: {e}")))?;

    let cookie = session_cookie(
        "seahub-session",
        &session_token,
        Some(ttl_days * 86400),
        state.config.server.secure_cookies(),
    );

    Ok((
        StatusCode::FOUND,
        [("Location", "/libraries/"), ("Set-Cookie", &cookie)],
    )
        .into_response())
}

// ─── Password Reset ─────────────────────────────────────────────────────────

#[derive(Template)]
#[template(path = "page/password_reset_form.html")]
pub struct PasswordResetFormTemplate {
    pub urls: &'static crate::static_assets::TemplateUrls,
    pub error: Option<String>,
}

#[derive(Template)]
#[template(path = "page/password_reset_done.html")]
pub struct PasswordResetDoneTemplate {
    pub urls: &'static crate::static_assets::TemplateUrls,
    pub reset_url: Option<String>,
}

#[derive(Template)]
#[template(path = "page/password_reset_confirm.html")]
pub struct PasswordResetConfirmTemplate {
    pub urls: &'static crate::static_assets::TemplateUrls,
    pub error: Option<String>,
    /// Whether the token is valid (show form) or invalid (show error).
    pub valid: bool,
}

#[derive(Template)]
#[template(path = "page/password_reset_complete.html")]
pub struct PasswordResetCompleteTemplate {
    pub urls: &'static crate::static_assets::TemplateUrls,
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
pub async fn password_reset_page() -> Result<Html<String>, AppError> {
    let tpl = PasswordResetFormTemplate {
        urls: crate::static_assets::template_urls(),
        error: None,
    };
    let html = tpl
        .render()
        .map_err(|e| AppError::internal(e.to_string()))?;
    Ok(Html(html))
}

/// POST /accounts/password/reset/ — generate a reset token and display the link.
pub async fn password_reset(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Form(form): Form<PasswordResetForm>,
) -> Result<Html<String>, AppError> {
    // CSRF: validate Origin/Referer.
    let origin = state.config.server.site_url_origin();
    if !crate::auth::csrf::validate_origin(&headers, &origin) {
        return Ok(Html(String::new()));
    }

    // Rate limit: per IP.
    let client_ip = headers
        .get("x-forwarded-for")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.split(',').next().map(|s| s.trim().to_string()))
        .or_else(|| {
            headers
                .get("x-real-ip")
                .and_then(|v| v.to_str().ok())
                .map(|s| s.to_string())
        })
        .unwrap_or_else(|| "unknown".to_string());
    let rl_key = format!("password_reset:{}", client_ip);
    if state.password_reset_limiter.is_limited(&rl_key) {
        // Show the done page silently to prevent enumeration.
        let tpl = PasswordResetDoneTemplate {
            urls: crate::static_assets::template_urls(),
            reset_url: None,
        };
        let html = tpl
            .render()
            .map_err(|e| AppError::internal(e.to_string()))?;
        return Ok(Html(html));
    }
    state.password_reset_limiter.record_attempt(&rl_key);

    let db = state.db.as_ref();

    // Look up the user (fail silently to prevent enumeration).
    let user_record = user::Entity::find()
        .filter(user::Column::Email.eq(&form.email))
        .one(db)
        .await?;

    let reset_url = if let Some(user) = user_record {
        let now = chrono::Utc::now().timestamp();
        let (raw_token, token_hash) = crate::auth::password_reset::generate_reset_token();

        let token_model = crate::entity::password_reset_token::ActiveModel {
            id: sea_orm::NotSet,
            user_id: Set(user.id),
            token_hash: Set(token_hash),
            created_at: Set(now),
            expires_at: Set(now + crate::auth::password_reset::RESET_TOKEN_TTL_SECONDS),
            used: Set(false),
        };

        token_model.insert(db).await?;

        let base = state.config.server.site_url.trim_end_matches('/');
        let link = format!("{}/accounts/password/reset/{}/", base, raw_token);
        tracing::info!("Password reset link generated for user {}", user.email);
        Some(link)
    } else {
        // User not found — still show the same success page (no enumeration).
        tracing::info!("Password reset requested for unknown email: {}", form.email);
        None
    };

    let tpl = PasswordResetDoneTemplate {
        urls: crate::static_assets::template_urls(),
        reset_url,
    };
    let html = tpl
        .render()
        .map_err(|e| AppError::internal(e.to_string()))?;
    Ok(Html(html))
}

/// GET /accounts/password/reset/done/ — show confirmation (accessed directly, no URL).
pub async fn password_reset_done() -> Result<Html<String>, AppError> {
    let tpl = PasswordResetDoneTemplate {
        urls: crate::static_assets::template_urls(),
        reset_url: None,
    };
    let html = tpl
        .render()
        .map_err(|e| AppError::internal(e.to_string()))?;
    Ok(Html(html))
}

/// Validate a raw reset token, returning the token record if valid.
async fn validate_reset_token(
    db: &sea_orm::DatabaseConnection,
    raw_token: &str,
) -> Result<Option<crate::entity::password_reset_token::Model>, AppError> {
    use crate::entity::password_reset_token;
    use sea_orm::ColumnTrait;

    let token_hash = crate::auth::password_reset::hash_token(raw_token);
    let record = password_reset_token::Entity::find()
        .filter(password_reset_token::Column::TokenHash.eq(&token_hash))
        .one(db)
        .await?;

    let record = match record {
        Some(r) => r,
        None => return Ok(None),
    };

    let now = chrono::Utc::now().timestamp();

    if record.used || record.expires_at <= now {
        return Ok(None);
    }

    Ok(Some(record))
}

/// GET /accounts/password/reset/{token}/ — show the new password form or error.
pub async fn password_reset_confirm_page(
    State(state): State<Arc<AppState>>,
    Path(token): Path<String>,
) -> Result<Html<String>, AppError> {
    let db = state.db.as_ref();
    let valid = validate_reset_token(db, &token).await?.is_some();

    let tpl = PasswordResetConfirmTemplate {
        urls: crate::static_assets::template_urls(),
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
    if !crate::auth::csrf::validate_origin(&headers, &origin) {
        return Ok(StatusCode::FORBIDDEN.into_response());
    }

    let db = state.db.as_ref();

    // Validate the token.
    let record = match validate_reset_token(db, &token).await? {
        Some(r) => r,
        None => {
            let tpl = PasswordResetConfirmTemplate {
                urls: crate::static_assets::template_urls(),
                error: Some("This reset link is invalid or has expired.".to_string()),
                valid: false,
            };
            let html = tpl
                .render()
                .map_err(|e| AppError::internal(e.to_string()))?;
            return Ok((StatusCode::OK, Html(html)).into_response());
        }
    };

    // Validate passwords match.
    if form.password1 != form.password2 {
        let tpl = PasswordResetConfirmTemplate {
            urls: crate::static_assets::template_urls(),
            error: Some("Passwords do not match.".to_string()),
            valid: true,
        };
        let html = tpl
            .render()
            .map_err(|e| AppError::internal(e.to_string()))?;
        return Ok((StatusCode::OK, Html(html)).into_response());
    }

    // Validate password strength.
    let cfg = &state.config.auth;
    if let Err(msg) = validate_password(
        &form.password1,
        cfg.password_min_length,
        cfg.require_strong_password,
    ) {
        let tpl = PasswordResetConfirmTemplate {
            urls: crate::static_assets::template_urls(),
            error: Some(msg),
            valid: true,
        };
        let html = tpl
            .render()
            .map_err(|e| AppError::internal(e.to_string()))?;
        return Ok((StatusCode::OK, Html(html)).into_response());
    }

    // Update the user's password.
    let new_hash = crate::auth::password::hash_password(
        &form.password1,
        state.config.auth.password_hash_iterations,
    );
    let mut user_active: user::ActiveModel = user::Entity::find_by_id(record.user_id)
        .one(db)
        .await?
        .ok_or_else(|| AppError::Internal("User not found.".to_string()))?
        .into();
    user_active.password_hash = Set(new_hash);
    user_active.update(db).await?;

    let user_id = record.user_id;

    // Mark token as used.
    let mut token_active: crate::entity::password_reset_token::ActiveModel = record.into();
    token_active.used = Set(true);
    token_active.update(db).await?;

    // Delete all session tokens for this user (force re-login).
    api_token::Entity::delete_many()
        .filter(api_token::Column::UserId.eq(user_id))
        .exec(db)
        .await?;

    Ok((
        StatusCode::FOUND,
        [("Location", "/accounts/password/reset/complete/")],
    )
        .into_response())
}

/// GET /accounts/password/reset/complete/ — show success page.
pub async fn password_reset_complete() -> Result<Html<String>, AppError> {
    let tpl = PasswordResetCompleteTemplate {
        urls: crate::static_assets::template_urls(),
    };
    let html = tpl
        .render()
        .map_err(|e| AppError::internal(e.to_string()))?;
    Ok(Html(html))
}
