/// GET /mobile-login/
///
/// Auto-login for the mobile clients' WebView. The Android/iOS clients open
/// wiki and file pages by first requesting `/mobile-login/?next=<full-url>`
/// with an `Authorization: Token <api-token>` header. This validates the API
/// token, establishes a `seahub-session` cookie, and redirects to `next`
/// (mirrors seahub's `views/mobile.py::mobile_login`).
use axum::{
    extract::{Query, State},
    http::{HeaderMap, StatusCode, header},
    response::{IntoResponse, Redirect},
};
use std::collections::HashMap;
use std::sync::Arc;

use crate::AppState;
use crate::repository::api_token::CreateSessionTokenParams;
use crate::service::auth::token::generate_api_token;
use crate::ui::client_login::resolve_next;
use base::error::AppError;

pub async fn mobile_login(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(params): Query<HashMap<String, String>>,
) -> Result<impl IntoResponse, AppError> {
    let next = resolve_next(params.get("next").map(String::as_str));

    // Extract and validate `Authorization: Token <token>`.
    let token_str = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Token "))
        .map(str::trim)
        .ok_or(AppError::Unauthorized)?;

    let token_record = state
        .repos
        .api_token
        .find_by_token(token_str)
        .await
        .map_err(|_| AppError::internal("database error"))?;

    // Any failure after this point just falls back to the unauthenticated
    // `next` target (seahub renders an error page; we redirect instead).
    let Some(token_record) = token_record else {
        return Ok(Redirect::to(&next).into_response());
    };
    if let Some(expires_at) = token_record.expires_at {
        let now = chrono::Utc::now().timestamp();
        if now > expires_at {
            return Ok(Redirect::to(&next).into_response());
        }
    }

    let user_record = state
        .repos
        .user
        .find_by_id(token_record.user_id)
        .await
        .map_err(|_| AppError::internal("database error"))?;
    let Some(user_record) = user_record else {
        return Ok(Redirect::to(&next).into_response());
    };
    if !user_record.is_active {
        return Ok(Redirect::to(&next).into_response());
    }

    // Generate a fresh session token and store it (same shape as the
    // `seahub-session` cookie used by the rest of the web UI).
    let session_token = generate_api_token();
    let ttl_days = state.config.auth.api_token_ttl_days;
    let now = chrono::Utc::now().timestamp();
    let expires_at = now + (ttl_days as i64 * 86400);

    state
        .repos
        .api_token
        .create_session_token(CreateSessionTokenParams {
            user_id: user_record.id,
            token: session_token.clone(),
            created_at: now,
            expires_at: Some(expires_at),
            device_id: None,
            platform: None,
            device_name: None,
            client_version: None,
        })
        .await
        .map_err(|e| AppError::internal(format!("failed to create session token: {e}")))?;

    let secure = if state.config.server.site_url.starts_with("https") {
        "; Secure"
    } else {
        ""
    };
    let cookie = format!(
        "seahub-session={}; HttpOnly; SameSite=Lax; Path=/; Max-Age={}{}",
        session_token,
        ttl_days * 86400,
        secure,
    );
    let csrf_cookie = crate::service::auth::csrf::csrf_cookie_header(
        &state.csrf_secret,
        &session_token,
        state.config.server.secure_cookies(),
        Some(ttl_days * 86400),
    );

    let mut resp_headers = HeaderMap::new();
    resp_headers.insert(
        header::LOCATION,
        axum::http::HeaderValue::from_str(&next)
            .map_err(|_| AppError::internal("Failed to create location header"))?,
    );
    resp_headers.append(
        header::SET_COOKIE,
        cookie
            .parse::<axum::http::HeaderValue>()
            .map_err(|_| AppError::internal("Failed to create session cookie header"))?,
    );
    resp_headers.append(
        header::SET_COOKIE,
        csrf_cookie
            .parse::<axum::http::HeaderValue>()
            .map_err(|_| AppError::internal("Failed to create CSRF cookie header"))?,
    );

    Ok((StatusCode::FOUND, resp_headers).into_response())
}
