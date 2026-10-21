/// Web UI auth handlers — login page, login submission, TOTP verification, logout.
use askama::Template;
use axum::{
    Form,
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
    response::{Html, IntoResponse, Response},
};
use serde::Deserialize;
use std::sync::Arc;

use crate::AppState;
use crate::domain::session_source::SessionSource;
use crate::i18n::I18n;
use crate::repository::api_token::CreateSessionTokenParams;
use crate::service::auth::email_verification::{EmailVerificationService, VerificationOutcome};
use crate::service::auth::password::{
    dummy_password_hash, hash_password_async, needs_rehash, verify_password_async,
};
use crate::service::auth::password_reset::PasswordResetService;
use crate::service::auth::pow::{PowPurpose, PowView, bits_for};
use crate::service::auth::registration::{RegistrationParams, RegistrationService};
use crate::service::auth::token::generate_api_token;
use crate::service::auth::totp::TotpManager;
use crate::ui::client_login::resolve_next;
use crate::ui::user_agent;
use base::error::AppError;

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
    /// Show/hide the "Create account" link; the POST handler enforces the same
    /// flag, so a disabled flow cannot be reached by navigating directly.
    pub registration_available: bool,
    /// URL to redirect to after login (kept as a hidden form field).
    pub next: String,
    /// Proof-of-work challenge, when the current policy asks for one.
    pub pow: Option<PowView>,
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

/// `?next=` carried on the login pages (e.g. the SSO local-browser bounce redirect).
#[derive(Deserialize)]
pub struct LoginNextQuery {
    pub next: Option<String>,
    /// Why the visitor was sent back, as a key listed in `LOGIN_ERROR_KEYS`; any
    /// other value is ignored, so the banner cannot reflect arbitrary text.
    pub err: Option<String>,
}

/// Reasons the sign-in form will display. The 2FA step has no page to report a
/// dead pending session on, so it hands the reason here.
const LOGIN_ERROR_KEYS: [&str; 4] = [
    "auth.session_expired",
    "auth.session_expired_alt",
    "auth.too_many_verification",
    "auth.two_factor_not_configured",
];

/// Translate `?err=` from a redirect, if it names a message we defined.
fn login_error(query: &LoginNextQuery, t: &I18n) -> Option<String> {
    let key = query.err.as_deref()?;
    LOGIN_ERROR_KEYS
        .contains(&key)
        .then(|| t.tr(key).to_string())
}

#[derive(Deserialize)]
pub struct LoginForm {
    pub email: String,
    pub password: String,
    /// "1" if the "Remember me" checkbox was checked.
    pub remember_me: Option<String>,
    /// Hidden form field echoing the original `?next=` query value.
    pub next: Option<String>,
    /// Proof-of-work challenge id and the browser-computed nonce.
    pub pow_id: Option<String>,
    pub pow_nonce: Option<String>,
}

#[derive(Deserialize)]
pub struct TwoFactorAuthForm {
    pub code: String,
    /// Hidden form field echoing the original `?next=` query value.
    pub next: Option<String>,
}

/// Whether the current policy asks the sign-in form for a proof-of-work.
///
/// The failure count is the same one the lockout sees, so an address that has been
/// failing gets a challenge before it is locked out. `trigger_after = 0` makes
/// adaptive behave like always.
fn login_requires_pow(state: &AppState, client_ip: &str, username: &str) -> bool {
    let auth = &state.config().auth;
    match auth.captcha_mode {
        infra::config::CaptchaMode::Off => false,
        infra::config::CaptchaMode::Always => true,
        infra::config::CaptchaMode::Adaptive => {
            let keys = infra::rate_limit::LoginKeys::new(client_ip, username);
            state.auth_limiters.login.failure_count(&keys) >= auth.captcha_trigger_after
        }
    }
}

/// Issue a sign-in challenge when the policy asks for one.
fn login_pow_challenge(state: &AppState, client_ip: &str, username: &str) -> Option<PowView> {
    if !login_requires_pow(state, client_ip, username) {
        return None;
    }
    let auth = &state.config().auth;
    let keys = infra::rate_limit::LoginKeys::new(client_ip, username);
    let failures = state.auth_limiters.login.failure_count(&keys);
    Some(
        state
            .pow
            .issue(PowPurpose::Login, bits_for(auth.captcha_bits(), failures)),
    )
}

/// Issue a challenge for a form that always asks while the feature is on.
fn form_pow_challenge(state: &AppState, purpose: PowPurpose) -> Option<PowView> {
    let auth = &state.config().auth;
    (auth.captcha_mode != infra::config::CaptchaMode::Off)
        .then(|| state.pow.issue(purpose, auth.captcha_bits()))
}

/// Verify the hidden fields; missing fields simply fail.
fn verify_pow(
    state: &AppState,
    id: &Option<String>,
    nonce: &Option<String>,
    purpose: PowPurpose,
) -> bool {
    match (id.as_deref(), nonce.as_deref()) {
        (Some(id), Some(nonce)) => state.pow.verify(id, nonce, purpose),
        _ => false,
    }
}

/// GET /accounts/login/ — render the login page.
pub async fn login_page(
    State(state): State<Arc<AppState>>,
    axum::extract::ConnectInfo(addr): axum::extract::ConnectInfo<std::net::SocketAddr>,
    headers: HeaderMap,
    Query(query): Query<LoginNextQuery>,
) -> Result<Html<String>, AppError> {
    let t = I18n::from_headers(&headers, &state.config().ui.default_language);
    let client_ip = crate::middleware::effective_client_ip(
        &addr,
        &headers,
        &state.config().server.trusted_proxies,
    );
    let tpl = LoginTemplate {
        urls: crate::static_assets::template_urls(),
        t,
        error: login_error(&query, t),
        remember_days: state.config().auth.api_token_ttl_days,
        enable_password_reset: password_reset_available(&state).await,
        registration_available: registration_available(&state),
        next: query.next.unwrap_or_default(),
        pow: login_pow_challenge(&state, &client_ip, ""),
    };
    let html = tpl
        .render()
        .map_err(|e| AppError::internal(e.to_string()))?;
    Ok(Html(html))
}

/// Whether the reset flow can deliver a link: the flag alone would offer a
/// "Forgot password?" page whose every submission silently does nothing.
async fn password_reset_available(state: &Arc<AppState>) -> bool {
    state.config().auth.enable_password_reset && state.mail.ready()
}

/// Tell the owner when a browser signs in for the first time.
///
/// Must be called *before* the new session row is created: an already-inserted
/// row matches itself, which would make every login look familiar.
async fn notify_new_browser(
    state: &Arc<AppState>,
    user_id: i32,
    headers: &HeaderMap,
    client_ip: &str,
) {
    use crate::service::mail::{MailKind, detect};

    if !state.mail.allows(MailKind::NewLogin) {
        return;
    }
    let agent = user_agent::from_headers(headers);
    let prior = match state.repos.api_token.list_sessions(user_id).await {
        Ok(sessions) => sessions,
        Err(e) => {
            tracing::warn!("could not list sessions for a new-login check: {e}");
            return;
        }
    };
    if !detect::is_new_browser(&prior, agent.as_deref()) {
        return;
    }
    state
        .mail
        .notify_new_login(user_id, agent.as_deref(), client_ip)
        .await;
}

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
            source: SessionSource::Web,
            // Never listed; a user agent here would only park a secret-adjacent value.
            user_agent: None,
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
    pow: Option<PowView>,
) -> Result<Html<String>, AppError> {
    let tpl = LoginTemplate {
        urls: crate::static_assets::template_urls(),
        t: I18n::from_headers(headers, &state.config().ui.default_language),
        error,
        remember_days: state.config().auth.api_token_ttl_days,
        enable_password_reset: password_reset_available(state).await,
        registration_available: registration_available(state),
        next: next.to_string(),
        pow,
    };
    let html = tpl
        .render()
        .map_err(|e| AppError::internal(e.to_string()))?;
    Ok(Html(html))
}

/// Whether self-registration can complete right now.
///
/// In invitation mode the code is the proof, so mail is irrelevant. In open mode
/// the verification link is the only way to activate the account, so an
/// installation without working mail must refuse the flow instead of creating
/// accounts that can never sign in.
fn registration_available(state: &Arc<AppState>) -> bool {
    let auth = &state.config().auth;
    auth.enable_registration && (auth.require_invitation || state.mail.ready())
}

/// Reject registration when the switch is off or, in open mode, when no
/// verification mail can be sent. Both the GET page and the POST use it, so the
/// condition cannot be bypassed by navigating there directly.
fn ensure_registration_available(state: &Arc<AppState>) -> Result<(), AppError> {
    if registration_available(state) {
        Ok(())
    } else {
        Err(AppError::NotFound("registration is disabled".into()))
    }
}

/// The sign-in response for a correct password on an unverified account.
///
/// No session is created. When mail is usable, a new link is sent unless one was
/// issued inside the cooldown window; the page is the same either way, so it does
/// not become an oracle for whether a send just happened.
async fn unverified_login_response(
    state: &Arc<AppState>,
    headers: &HeaderMap,
    user: &infra::entity::user::Model,
    client_ip: &str,
) -> Result<Response, AppError> {
    let t = I18n::from_headers(headers, &state.config().ui.default_language);

    if !state.mail.ready() {
        return render_login_page(
            state,
            headers,
            Some(t.tr("auth.verify_email_required_no_mail").to_string()),
            "",
            login_pow_challenge(state, client_ip, &user.email),
        )
        .await
        .map(|html| (StatusCode::OK, Html(html)).into_response());
    }

    let service = EmailVerificationService::new(state.repos.clone());
    let now = chrono::Utc::now().timestamp();
    let send = if service.recently_issued(user.id, now).await? {
        true
    } else {
        let link = service
            .issue(user.id, &state.config().server.site_url)
            .await?;
        match state
            .mail
            .send_email_verification(
                &user.email,
                user.id,
                user.language.as_deref(),
                &link,
                &user.nickname(),
                Some(client_ip),
            )
            .await
        {
            Ok(()) => true,
            Err(e) => {
                tracing::warn!(
                    "could not queue an email-verification link for user {}: {e}",
                    user.id
                );
                // Drop the token we just minted so an immediate retry is not
                // suppressed by the resend cooldown. The old link was already
                // replaced by issue(), so there is nothing to keep.
                if let Err(cleanup) = state
                    .repos
                    .email_verification_token
                    .delete_by_user(user.id)
                    .await
                {
                    tracing::warn!(
                        "could not clear the unusable verification token for user {}: {cleanup}",
                        user.id
                    );
                }
                false
            }
        }
    };

    if !send {
        return render_login_page(
            state,
            headers,
            Some(t.tr("auth.verification_email_failed").to_string()),
            "",
            login_pow_challenge(state, client_ip, &user.email),
        )
        .await
        .map(|html| (StatusCode::OK, Html(html)).into_response());
    }

    let tpl = VerifyEmailSentTemplate {
        urls: crate::static_assets::template_urls(),
        t,
        email: user.email.clone(),
    };
    let html = tpl
        .render()
        .map_err(|e| AppError::internal(e.to_string()))?;
    Ok((StatusCode::OK, Html(html)).into_response())
}

/// Post /accounts/login/ — authenticate user, handle 2FA if enabled.
pub async fn login(
    State(state): State<Arc<AppState>>,
    axum::extract::ConnectInfo(addr): axum::extract::ConnectInfo<std::net::SocketAddr>,
    headers: HeaderMap,
    Query(query): Query<LoginNextQuery>,
    Form(form): Form<LoginForm>,
) -> Result<impl IntoResponse, AppError> {
    // The hidden `next` field carries `?next=`; fall back to the query if absent.
    let tenant_origin = state.config().server.site_url_origin();
    let next = resolve_next(
        form.next.as_deref().or(query.next.as_deref()),
        &tenant_origin,
    );

    // CSRF: validate Origin/Referer.
    let origin = state.config().server.site_url_origin();
    if !crate::service::auth::csrf::validate_origin(&headers, &origin) {
        return render_login_page(
            &state,
            &headers,
            Some(
                I18n::from_headers(&headers, &state.config().ui.default_language)
                    .tr("auth.invalid_origin")
                    .to_string(),
            ),
            &next,
            None,
        )
        .await
        .map(|html| (StatusCode::FORBIDDEN, Html(html)).into_response());
    }

    // Client IP for rate limiting: TCP peer, with XFF trusted only from configured proxies.
    let client_ip = crate::middleware::effective_client_ip(
        &addr,
        &headers,
        &state.config().server.trusted_proxies,
    );

    // Throttle keys: the client address, the (address, account) pair, and the
    // limiter's per-address distinct-account count. No username-only key — that
    // would let anyone lock a victim out of their own account.
    let login_keys = infra::rate_limit::LoginKeys::new(&client_ip, &form.email);

    if state.auth_limiters.login.is_login_blocked(&login_keys) {
        return render_login_page(
            &state,
            &headers,
            Some(
                I18n::from_headers(&headers, &state.config().ui.default_language)
                    .tr("auth.too_many_attempts")
                    .to_string(),
            ),
            &next,
            None,
        )
        .await
        .map(|html| (StatusCode::TOO_MANY_REQUESTS, Html(html)).into_response());
    }

    // The proof-of-work comes before any password work: a missing or wrong answer
    // must not buy a PBKDF2 run, and it is not counted as a credential failure.
    if login_requires_pow(&state, &client_ip, &form.email)
        && !verify_pow(&state, &form.pow_id, &form.pow_nonce, PowPurpose::Login)
    {
        return render_login_page(
            &state,
            &headers,
            Some(
                I18n::from_headers(&headers, &state.config().ui.default_language)
                    .tr("auth.pow_invalid")
                    .to_string(),
            ),
            &next,
            login_pow_challenge(&state, &client_ip, &form.email),
        )
        .await
        .map(|html| (StatusCode::OK, Html(html)).into_response());
    }

    let user_record = state.repos.user.find_by_email(&form.email).await?;

    // Verify against a cost-matched dummy hash when the row is missing, and check
    // the password before `is_active`: otherwise a disabled account answers in
    // ~1ms while a wrong password spends ~100-300ms, a reliable "this email
    // exists but is disabled" oracle.
    let password_ok = match &user_record {
        Some(u) => {
            verify_password_async(
                form.password.clone(),
                u.password_hash.clone(),
                state.config().auth.password_hash_iterations,
            )
            .await
        }
        None => {
            let _ = verify_password_async(
                form.password.clone(),
                dummy_password_hash(state.config().auth.password_hash_iterations),
                state.config().auth.password_hash_iterations,
            )
            .await;
            false
        }
    };

    // Same generic error for wrong password, disabled account and unknown email,
    // to avoid user enumeration (matching seahub).
    let Some(user_record) = user_record.filter(|u| password_ok && u.is_active) else {
        state.auth_limiters.login.record_login_failure(&login_keys);
        return render_login_page(
            &state,
            &headers,
            Some(
                I18n::from_headers(&headers, &state.config().ui.default_language)
                    .tr("auth.incorrect_credentials")
                    .to_string(),
            ),
            &next,
            login_pow_challenge(&state, &client_ip, &form.email),
        )
        .await
        .map(|html| (StatusCode::OK, Html(html)).into_response());
    };

    // Success forgives this address/pair but keeps the distinct-account spray history.
    state.auth_limiters.login.clear_login_failure(&login_keys);

    // A correct password is not enough for an unverified account: no session is
    // created and no last-login is recorded. Instead, offer a fresh link, but only
    // after the cooldown, so signing in right after registering does not invalidate
    // the link that was just mailed.
    if !user_record.email_verified {
        return unverified_login_response(&state, &headers, &user_record, &client_ip).await;
    }

    // Upgrade a stored hash that predates the embedded cost or used another one.
    // Best-effort: a failed write must not block a correct password.
    if needs_rehash(
        &user_record.password_hash,
        state.config().auth.password_hash_iterations,
    ) {
        let upgraded = hash_password_async(
            form.password.clone(),
            state.config().auth.password_hash_iterations,
        )
        .await;
        if let Err(e) = state
            .repos
            .user
            .update_password(user_record.id, upgraded)
            .await
        {
            tracing::warn!(
                "failed to upgrade password hash for user {}: {e}",
                user_record.id
            );
        }
    }

    let two_fa = state.repos.user_2fa.find_by_user_id(user_record.id).await?;

    if let Some(tfa) = two_fa
        && tfa.enabled
    {
        let pending_token = create_pending_token(&state.repos, user_record.id).await?;
        let cookie = session_cookie(
            "seahub-session-pending",
            &pending_token,
            Some(300),
            state.config().server.secure_cookies(),
        );

        // Thread `next` through 2FA so the user still lands on the original destination.
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

    // Before the session is stored, so "have I seen this browser?" can be asked.
    notify_new_browser(&state, user_record.id, &headers, &client_ip).await;

    let session_token = generate_api_token();
    let now = chrono::Utc::now().timestamp();

    // "Remember me" checkbox: checked → long-lived session; unchecked → session cookie.
    let is_remembered =
        form.remember_me.as_deref() == Some("1") || form.remember_me.as_deref() == Some("on");

    let ttl_days = state.config().auth.api_token_ttl_days;
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
            source: SessionSource::Web,
            user_agent: user_agent::from_headers(&headers),
        })
        .await
        .map_err(|e| AppError::internal(format!("failed to create session token: {e}")))?;

    state
        .repos
        .user
        .touch_last_login(user_record.id, now)
        .await?;

    let secure_cookies = state.config().server.secure_cookies();
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
        t: I18n::from_headers(&headers, &state.config().ui.default_language),
        error: None,
        next: query.next.unwrap_or_default(),
    };
    let html = tpl
        .render()
        .map_err(|e| AppError::internal(e.to_string()))?;
    Ok(Html(html))
}

/// Send the visitor back to the sign-in form with the reason: a pending 2FA
/// session that cannot be used has no page of its own. `next` rides along and
/// the dead pending cookie is cleared so the browser stops sending it.
fn back_to_login(
    next: &str,
    reason_key: Option<&str>,
    secure_cookies: bool,
) -> Result<Response, AppError> {
    let mut location = String::from("/accounts/login/");
    let mut sep = "?";
    if let Some(key) = reason_key {
        location.push_str(sep);
        location.push_str("err=");
        location.push_str(key);
        sep = "&";
    }
    if !next.is_empty() {
        location.push_str(sep);
        location.push_str("next=");
        location.extend(percent_encoding::utf8_percent_encode(
            next,
            percent_encoding::NON_ALPHANUMERIC,
        ));
    }

    let mut resp_headers = ::axum::http::HeaderMap::new();
    resp_headers.insert(
        ::axum::http::header::LOCATION,
        ::axum::http::HeaderValue::from_str(&location)
            .map_err(|_| AppError::internal("Failed to create location header"))?,
    );
    resp_headers.append(
        ::axum::http::header::SET_COOKIE,
        session_cookie("seahub-session-pending", "", Some(0), secure_cookies)
            .parse::<::axum::http::HeaderValue>()
            .map_err(|_| AppError::internal("Failed to create pending cookie header"))?,
    );

    Ok((StatusCode::FOUND, resp_headers).into_response())
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
    let tenant_origin = state.config().server.site_url_origin();
    let next = resolve_next(
        form.next.as_deref().or(query.next.as_deref()),
        &tenant_origin,
    );

    // CSRF: validate Origin/Referer.
    let origin = state.config().server.site_url_origin();
    if !crate::service::auth::csrf::validate_origin(&headers, &origin) {
        return Err(AppError::BadRequest(
            I18n::from_headers(&headers, &state.config().ui.default_language)
                .tr("auth.invalid_origin")
                .to_string(),
        ));
    }

    // Client IP for rate limiting: TCP peer address (not spoofable XFF).
    let client_ip = crate::middleware::effective_client_ip(
        &addr,
        &headers,
        &state.config().server.trusted_proxies,
    );

    // From here on every failure means the pending 2FA session cannot be used,
    // so the reason goes back to the sign-in form rather than a generic error page.
    let secure_cookies = state.config().server.secure_cookies();

    let pending_token = headers
        .get("Cookie")
        .and_then(|v| v.to_str().ok())
        .and_then(|cookie_str| {
            cookie_str
                .split(';')
                .map(|s| s.trim())
                .find(|s| s.starts_with("seahub-session-pending="))
                .and_then(|s| s.strip_prefix("seahub-session-pending="))
        });
    let Some(pending_token) = pending_token else {
        return back_to_login(&next, Some("auth.session_expired"), secure_cookies);
    };

    let token_record = match state.repos.api_token.find_by_token(pending_token).await {
        Ok(Some(record)) => record,
        Ok(None) => {
            return back_to_login(&next, Some("auth.session_expired_alt"), secure_cookies);
        }
        Err(_) => return Err(AppError::internal("database error")),
    };

    if let Some(expires_at) = token_record.expires_at {
        let now = chrono::Utc::now().timestamp();
        if now > expires_at {
            let _ = state.repos.api_token.delete_by_token(pending_token).await;
            return back_to_login(&next, Some("auth.session_expired"), secure_cookies);
        }
    }

    // Only a 2FA-pending token may be presented here. Without this check any
    // live account token placed in the `seahub-session-pending` cookie is
    // accepted as evidence that the password step already happened, which is
    // the precondition this page exists to establish (every other consumer of
    // an API token rejects `is_pending`).
    if !token_record.is_pending {
        return back_to_login(&next, Some("auth.session_expired_alt"), secure_cookies);
    }

    let user_id = token_record.user_id;

    // Rate limit: per user+IP.
    let totp_key = format!("totp:{}:{}", user_id, client_ip);
    if state.auth_limiters.totp.is_limited(&totp_key) {
        // Delete pending token to force re-login
        let _ = state.repos.api_token.delete_by_token(pending_token).await;
        return back_to_login(&next, Some("auth.too_many_verification"), secure_cookies);
    }

    let two_fa = state.repos.user_2fa.find_by_user_id(user_id).await?;
    let Some(two_fa) = two_fa else {
        // The config was removed between the password step and now, so signing
        // in again simply passes without a second factor.
        let _ = state.repos.api_token.delete_by_token(pending_token).await;
        return back_to_login(
            &next,
            Some("auth.two_factor_not_configured"),
            secure_cookies,
        );
    };

    if !two_fa.enabled {
        // 2FA was disabled since the pending token was created — proceed to login
        let _ = state.repos.api_token.delete_by_token(pending_token).await;
        return back_to_login(&next, None, secure_cookies);
    }

    let user_record = state
        .repos
        .user
        .find_by_id(user_id)
        .await?
        .ok_or(AppError::Unauthorized)?;

    let totp = TotpManager::create_totp(&two_fa.totp_secret, &user_record.email, "Nanofile")
        .map_err(|e| AppError::internal(e.to_string()))?;

    let code_valid =
        TotpManager::verify_and_consume(&state.repos, user_id, &totp, &form.code).await;

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
            t: I18n::from_headers(&headers, &state.config().ui.default_language),
            error: Some(
                I18n::from_headers(&headers, &state.config().ui.default_language)
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

    state.auth_limiters.totp.clear(&totp_key);
    let _ = state.repos.api_token.delete_by_token(pending_token).await;

    // The second factor was satisfied below; from the owner's point of view this
    // is still the first sign-in from this browser.
    notify_new_browser(&state, user_id, &headers, &client_ip).await;

    let session_token = generate_api_token();
    let now = chrono::Utc::now().timestamp();
    let ttl_days = state.config().auth.api_token_ttl_days;
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
            source: SessionSource::Web,
            user_agent: user_agent::from_headers(&headers),
        })
        .await
        .map_err(|e| AppError::internal(format!("failed to create session token: {e}")))?;

    state.repos.user.touch_last_login(user_id, now).await?;

    let secure_cookies = state.config().server.secure_cookies();
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
    // Logging out is a state change served on GET, so a cross-site
    // `<img src="/accounts/logout/">` could force it. Any browser navigation
    // carries `Sec-Fetch-Site`, and a cross-site one is refused; the token is
    // simply left in place (no error page, so nothing is disclosed). Keep GET so
    // existing links and bookmarks keep working.
    if headers
        .get("sec-fetch-site")
        .and_then(|v| v.to_str().ok())
        .is_some_and(|site| !matches!(site, "none" | "same-origin"))
    {
        let mut resp = axum::response::Redirect::to("/libraries/").into_response();
        resp.headers_mut().insert(
            axum::http::header::LOCATION,
            axum::http::HeaderValue::from_static("/libraries/"),
        );
        return Ok(resp);
    }

    if let Some(cookie_str) = headers.get("Cookie").and_then(|v| v.to_str().ok())
        && let Some(token) = cookie_str
            .split(';')
            .map(|s| s.trim())
            .find(|s| s.starts_with("seahub-session="))
            .and_then(|s| s.strip_prefix("seahub-session="))
    {
        let _ = state.repos.api_token.delete_by_token(token).await;
    }

    let secure_cookies = state.config().server.secure_cookies();
    let clear_cookie = session_cookie("seahub-session", "", Some(0), secure_cookies);
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

#[derive(Template)]
#[template(path = "page/register.html")]
pub struct RegisterTemplate {
    pub urls: &'static crate::static_assets::TemplateUrls,
    pub t: &'static I18n,
    pub error: Option<String>,
    /// Whether the form asks for an invitation code. Open registration hides the
    /// field and shows the verification-mail note instead.
    pub require_invitation: bool,
    /// Proof-of-work challenge, when the feature is on.
    pub pow: Option<PowView>,
}

#[derive(Deserialize)]
pub struct RegisterForm {
    pub email: String,
    pub password1: String,
    pub password2: String,
    /// Absent in open mode; also accepted (and ignored) when an old client sends it.
    pub invitation_code: Option<String>,
    /// Proof-of-work challenge id and the browser-computed nonce.
    pub pow_id: Option<String>,
    pub pow_nonce: Option<String>,
}

/// GET /accounts/register/ — render the registration form.
pub async fn register_page(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Result<Html<String>, AppError> {
    ensure_registration_available(&state)?;

    let tpl = RegisterTemplate {
        urls: crate::static_assets::template_urls(),
        t: I18n::from_headers(&headers, &state.config().ui.default_language),
        error: None,
        require_invitation: state.config().auth.require_invitation,
        pow: form_pow_challenge(&state, PowPurpose::Register),
    };
    let html = tpl
        .render()
        .map_err(|e| AppError::internal(e.to_string()))?;
    Ok(Html(html))
}

/// Re-render the registration form with the reason the submission was refused.
/// The form is a plain HTML POST, so the reason has to come back as a page.
fn register_error(
    state: &AppState,
    headers: &HeaderMap,
    message: impl AsRef<str>,
) -> Result<Response, AppError> {
    let tpl = RegisterTemplate {
        urls: crate::static_assets::template_urls(),
        t: I18n::from_headers(headers, &state.config().ui.default_language),
        error: Some(message.as_ref().to_string()),
        require_invitation: state.config().auth.require_invitation,
        pow: form_pow_challenge(state, PowPurpose::Register),
    };
    let html = tpl
        .render()
        .map_err(|e| AppError::internal(e.to_string()))?;
    Ok(Html(html).into_response())
}

/// POST /accounts/register/ — validate invitation code and create account.
pub async fn register(
    State(state): State<Arc<AppState>>,
    axum::extract::ConnectInfo(addr): axum::extract::ConnectInfo<std::net::SocketAddr>,
    headers: HeaderMap,
    Form(form): Form<RegisterForm>,
) -> Result<impl IntoResponse, AppError> {
    // CSRF: validate Origin/Referer.
    let origin = state.config().server.site_url_origin();
    if !crate::service::auth::csrf::validate_origin(&headers, &origin) {
        return register_error(
            &state,
            &headers,
            I18n::from_headers(&headers, &state.config().ui.default_language)
                .tr("auth.invalid_origin"),
        );
    }

    // Registration closed: refuse before any invitation or password work.
    ensure_registration_available(&state)?;

    // Rate limit: per IP (TCP peer, not spoofable XFF).
    let client_ip = crate::middleware::effective_client_ip(
        &addr,
        &headers,
        &state.config().server.trusted_proxies,
    );
    let rl_key = format!("register:{}", client_ip);
    if state.auth_limiters.registration.is_limited(&rl_key) {
        return register_error(
            &state,
            &headers,
            I18n::from_headers(&headers, &state.config().ui.default_language)
                .tr("auth.registration_too_many"),
        );
    }
    state.auth_limiters.registration.record_attempt(&rl_key);

    // The challenge is checked before any password or invitation work. A failed
    // answer re-renders the form with a fresh challenge.
    if state.config().auth.captcha_mode != infra::config::CaptchaMode::Off
        && !verify_pow(&state, &form.pow_id, &form.pow_nonce, PowPurpose::Register)
    {
        return register_error(
            &state,
            &headers,
            I18n::from_headers(&headers, &state.config().ui.default_language)
                .tr("auth.pow_invalid"),
        );
    }

    if form.password1 != form.password2 {
        return register_error(
            &state,
            &headers,
            I18n::from_headers(&headers, &state.config().ui.default_language)
                .tr("auth.passwords_mismatch"),
        );
    }

    let cfg = &state.config().auth;

    let reg_service = RegistrationService::new(state.repos.clone());
    let require_invitation = cfg.require_invitation;
    let result = match reg_service
        .register(RegistrationParams {
            email: form.email,
            password: form.password1,
            invitation_code: form.invitation_code,
            require_invitation,
            password_min_length: cfg.password_min_length as usize,
            require_strong_password: cfg.require_strong_password,
            password_hash_iterations: cfg.password_hash_iterations,
        })
        .await
    {
        Ok(result) => result,
        // A refused registration (taken address, spent code, weak password) is
        // the reader's to fix, so the form comes back with the reason; returning
        // the error would answer a plain HTML form with the wire protocol's JSON.
        Err(AppError::BadRequest(message)) => return register_error(&state, &headers, &message),
        Err(err) => return Err(err),
    };

    if !require_invitation {
        return finish_open_registration(&state, &headers, &result.user, &client_ip).await;
    }

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
            source: SessionSource::Web,
            user_agent: user_agent::from_headers(&headers),
        })
        .await
        .map_err(|e| AppError::internal(format!("failed to create session token: {e}")))?;

    state
        .repos
        .user
        .touch_last_login(result.user.id, now)
        .await?;

    let secure_cookies = state.config().server.secure_cookies();
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

/// Finish an open registration: mint a verification link, queue the mail, and
/// show the "check your inbox" page. Nothing is logged in here.
///
/// A failure to mint or queue is rolled back by deleting the account that was just
/// created: leaving an address registered with no way to activate it would make
/// the same address unusable for a retry.
async fn finish_open_registration(
    state: &Arc<AppState>,
    headers: &HeaderMap,
    user: &infra::entity::user::Model,
    client_ip: &str,
) -> Result<Response, AppError> {
    let service = EmailVerificationService::new(state.repos.clone());
    let sent = async {
        let link = service
            .issue(user.id, &state.config().server.site_url)
            .await?;
        state
            .mail
            .send_email_verification(
                &user.email,
                user.id,
                user.language.as_deref(),
                &link,
                &user.nickname(),
                Some(client_ip),
            )
            .await
    }
    .await;

    if let Err(e) = sent {
        tracing::warn!(
            "open registration for {} could not send a verification link: {e}; rolling back",
            user.id
        );
        if let Err(cleanup) = state.repos.user.delete_user(user.id).await {
            tracing::error!(
                "could not roll back unverified account {}: {cleanup}",
                user.id
            );
        }
        let message = I18n::from_headers(headers, &state.config().ui.default_language)
            .tr("auth.verification_email_failed")
            .to_string();
        return register_error(state, headers, message);
    }

    let tpl = VerifyEmailSentTemplate {
        urls: crate::static_assets::template_urls(),
        t: I18n::from_headers(headers, &state.config().ui.default_language),
        email: user.email.clone(),
    };
    let html = tpl
        .render()
        .map_err(|e| AppError::internal(e.to_string()))?;
    Ok((StatusCode::OK, Html(html)).into_response())
}

#[derive(Template)]
#[template(path = "page/verify_email_sent.html")]
pub struct VerifyEmailSentTemplate {
    pub urls: &'static crate::static_assets::TemplateUrls,
    pub t: &'static I18n,
    pub email: String,
}

#[derive(Template)]
#[template(path = "page/verify_email.html")]
pub struct VerifyEmailTemplate {
    pub urls: &'static crate::static_assets::TemplateUrls,
    pub t: &'static I18n,
    pub verified: bool,
}

/// GET /accounts/verify-email/{token}/ — claim a one-time link and mark the
/// account verified. No session is created; the page links to the login form.
pub async fn verify_email(
    State(state): State<Arc<AppState>>,
    Path(token): Path<String>,
    headers: HeaderMap,
) -> Result<Html<String>, AppError> {
    let service = EmailVerificationService::new(state.repos.clone());
    let verified = matches!(
        service.verify(&token).await?,
        VerificationOutcome::Verified { .. }
    );

    let tpl = VerifyEmailTemplate {
        urls: crate::static_assets::template_urls(),
        t: I18n::from_headers(&headers, &state.config().ui.default_language),
        verified,
    };
    let html = tpl
        .render()
        .map_err(|e| AppError::internal(e.to_string()))?;
    Ok(Html(html))
}

#[derive(Template)]
#[template(path = "page/password_reset_form.html")]
pub struct PasswordResetFormTemplate {
    pub urls: &'static crate::static_assets::TemplateUrls,
    pub t: &'static I18n,
    pub error: Option<String>,
    /// Proof-of-work challenge, when the feature is on.
    pub pow: Option<PowView>,
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
    /// Proof-of-work challenge id and the browser-computed nonce.
    pub pow_id: Option<String>,
    pub pow_nonce: Option<String>,
}

#[derive(Deserialize)]
pub struct PasswordResetConfirmForm {
    pub password1: String,
    pub password2: String,
}

/// Render the reset request form (fresh or with the reason it was refused).
fn render_password_reset_form(
    state: &AppState,
    headers: &HeaderMap,
    error: Option<String>,
) -> Result<Html<String>, AppError> {
    let tpl = PasswordResetFormTemplate {
        urls: crate::static_assets::template_urls(),
        t: I18n::from_headers(headers, &state.config().ui.default_language),
        error,
        pow: form_pow_challenge(state, PowPurpose::PasswordReset),
    };
    let html = tpl
        .render()
        .map_err(|e| AppError::internal(e.to_string()))?;
    Ok(Html(html))
}

/// GET /accounts/password/reset/ — show the reset request form. Answers 404 when
/// the flow cannot deliver a link: serving it anyway made a mail-less install
/// look like it had a working reset.
pub async fn password_reset_page(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Result<Html<String>, AppError> {
    if !password_reset_available(&state).await {
        return Err(AppError::NotFound("password reset is unavailable".into()));
    }
    render_password_reset_form(&state, &headers, None)
}

/// POST /accounts/password/reset/ — request a reset. The link is **never** in the
/// response; without an email backend no token is minted, so it cannot be used to
/// take over an account.
pub async fn password_reset(
    State(state): State<Arc<AppState>>,
    axum::extract::ConnectInfo(addr): axum::extract::ConnectInfo<std::net::SocketAddr>,
    headers: HeaderMap,
    Form(form): Form<PasswordResetForm>,
) -> Result<Html<String>, AppError> {
    // CSRF: validate Origin/Referer.
    let origin = state.config().server.site_url_origin();
    if !crate::service::auth::csrf::validate_origin(&headers, &origin) {
        return Ok(Html(String::new()));
    }

    // The feature switch and an email backend are both required.
    if !state.config().auth.enable_password_reset {
        let tpl = PasswordResetDoneTemplate {
            urls: crate::static_assets::template_urls(),
            t: I18n::from_headers(&headers, &state.config().ui.default_language),
        };
        let html = tpl
            .render()
            .map_err(|e| AppError::internal(e.to_string()))?;
        return Ok(Html(html));
    }

    // Email delivery is required to hand the link to the owner: minting a token
    // and echoing it back would let anyone reset any account. `ready()` is
    // stricter than `email.enabled` — it also requires an SMTP host and sender —
    // so a half-configured install mints nothing.
    if !state.mail.ready() {
        let tpl = PasswordResetDoneTemplate {
            urls: crate::static_assets::template_urls(),
            t: I18n::from_headers(&headers, &state.config().ui.default_language),
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
        &state.config().server.trusted_proxies,
    );
    let rl_key = format!("password_reset:{}", client_ip);
    if state.auth_limiters.password_reset.is_limited(&rl_key) {
        // Show the done page silently to prevent enumeration.
        let tpl = PasswordResetDoneTemplate {
            urls: crate::static_assets::template_urls(),
            t: I18n::from_headers(&headers, &state.config().ui.default_language),
        };
        let html = tpl
            .render()
            .map_err(|e| AppError::internal(e.to_string()))?;
        return Ok(Html(html));
    }
    state.auth_limiters.password_reset.record_attempt(&rl_key);

    // The challenge is checked before minting a token or queueing mail. A failed
    // answer re-renders the form with a fresh challenge; it reveals nothing about
    // whether the address exists.
    if state.config().auth.captcha_mode != infra::config::CaptchaMode::Off
        && !verify_pow(
            &state,
            &form.pow_id,
            &form.pow_nonce,
            PowPurpose::PasswordReset,
        )
    {
        return render_password_reset_form(
            &state,
            &headers,
            Some(
                I18n::from_headers(&headers, &state.config().ui.default_language)
                    .tr("auth.pow_invalid")
                    .to_string(),
            ),
        );
    }

    // Mint a token and mail it to the owner; the response deliberately carries no
    // link. The send is queued in the background rather than awaited: a known
    // address would otherwise take an SMTP round trip an unknown one does not,
    // and that timing difference is a user-enumeration oracle. Failures are
    // visible on /sysadmin/email/.
    let reset_service = PasswordResetService::new(state.repos.clone());
    let result = reset_service
        .create_reset_token(&form.email, &state.config().server.site_url)
        .await?;
    if let (Some(user_id), Some(reset_url)) = (result.user_id, result.reset_url) {
        state
            .mail
            .send_password_reset(
                &form.email,
                user_id,
                result.language.as_deref(),
                &reset_url,
                Some(&client_ip),
            )
            .await;
    }

    let tpl = PasswordResetDoneTemplate {
        urls: crate::static_assets::template_urls(),
        t: I18n::from_headers(&headers, &state.config().ui.default_language),
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
        t: I18n::from_headers(&headers, &state.config().ui.default_language),
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
    // `auth.enable_password_reset = false` must close the whole flow, not just hide the link.
    if !state.config().auth.enable_password_reset {
        return Err(AppError::NotFound("password reset is disabled".into()));
    }

    let reset_service = PasswordResetService::new(state.repos.clone());
    let valid = reset_service.validate_token(&token).await?.is_some();

    let tpl = PasswordResetConfirmTemplate {
        urls: crate::static_assets::template_urls(),
        t: I18n::from_headers(&headers, &state.config().ui.default_language),
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
    if !state.config().auth.enable_password_reset {
        return Err(AppError::NotFound("password reset is disabled".into()));
    }

    // CSRF: validate Origin/Referer.
    let origin = state.config().server.site_url_origin();
    if !crate::service::auth::csrf::validate_origin(&headers, &origin) {
        return Ok(StatusCode::FORBIDDEN.into_response());
    }

    let cfg = &state.config().auth;

    if form.password1 != form.password2 {
        let tpl = PasswordResetConfirmTemplate {
            urls: crate::static_assets::template_urls(),
            t: I18n::from_headers(&headers, &state.config().ui.default_language),
            error: Some(
                I18n::from_headers(&headers, &state.config().ui.default_language)
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

    // The token manager is threaded in so the reset drops outstanding in-memory capability URLs.
    let reset_service =
        PasswordResetService::with_token_manager(state.repos.clone(), state.token_manager.clone());
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
                t: I18n::from_headers(&headers, &state.config().ui.default_language),
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
        t: I18n::from_headers(&headers, &state.config().ui.default_language),
    };
    let html = tpl
        .render()
        .map_err(|e| AppError::internal(e.to_string()))?;
    Ok(Html(html))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn query(err: Option<&str>) -> LoginNextQuery {
        LoginNextQuery {
            next: None,
            err: err.map(str::to_string),
        }
    }

    #[test]
    fn login_error_accepts_only_the_reason_keys_it_defined() {
        let en = I18n::get(Some("en"));
        assert_eq!(
            login_error(&query(Some("auth.session_expired")), en).as_deref(),
            Some("Authentication session expired. Please log in again.")
        );
        assert_eq!(
            login_error(&query(Some("auth.too_many_verification")), en).as_deref(),
            Some("Too many verification attempts. Please log in again.")
        );
        // Everything else is dropped: the banner must not reflect arbitrary
        // text from the query string back into the page.
        assert_eq!(login_error(&query(Some("<b>call 555-0100</b>")), en), None);
        assert_eq!(login_error(&query(Some("no.such.key")), en), None);
        assert_eq!(login_error(&query(Some("auth.invalid_code")), en), None);
        assert_eq!(login_error(&query(None), en), None);
    }
}
