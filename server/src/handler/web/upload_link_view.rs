use askama::Template;
use axum::{
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
    response::{Html, IntoResponse, Response},
};
use std::collections::HashMap;
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

/// Cookie value marking that this browser session has supplied the correct
/// upload-link password. Mirrors seahub's `visited_ufs_{token}` session flag.
fn upload_link_cookie(token: &str) -> String {
    format!("visited_ufs_{token}=1; Path=/; Max-Age=86400; HttpOnly; SameSite=Lax")
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

    Ok(link)
}

/// Check whether the password in the request matches the stored hash.
async fn check_password(
    link: &infra::entity::upload_link::Model,
    params: &HashMap<String, String>,
    password_hash_iterations: u32,
) -> bool {
    let stored_hash = match link.password {
        Some(ref h) => h,
        None => return true, // no password required
    };

    let provided = params.get("password");

    match provided {
        Some(pwd) => {
            crate::service::auth::password::verify_password_async(
                pwd.clone(),
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
pub async fn upload_link_view(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(token): Path<String>,
    Query(params): Query<HashMap<String, String>>,
) -> Result<Response, AppError> {
    let link = validate_upload_link(&state, &token).await?;

    // Password check
    let pw_ok = check_password(&link, &params, state.config.auth.password_hash_iterations).await;
    // The password form POST sets `visited_ufs_{token}`; accept that cookie as
    // an unlock too, so the redirect back to /u/{token}/ isn't bounced to the
    // form again.
    let cookie_ok = link.password.is_some()
        && headers
            .get("cookie")
            .and_then(|v| v.to_str().ok())
            .is_some_and(|c| {
                c.split(';')
                    .map(|s| s.trim())
                    .any(|s| s == format!("visited_ufs_{token}=1"))
            });
    let unlocked = pw_ok || cookie_ok;

    // If password is required but not satisfied, show password form
    if !unlocked {
        let error = if params.contains_key("password") {
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
        && let Ok(value) = axum::http::HeaderValue::from_str(&upload_link_cookie(&token))
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
    Path(token): Path<String>,
    axum::Form(form): axum::Form<HashMap<String, String>>,
) -> Result<Response, AppError> {
    let link = validate_upload_link(&state, &token).await?;

    let password = form
        .get("password")
        .ok_or_else(|| AppError::BadRequest("password required".into()))?;

    let valid = crate::service::auth::password::verify_password_async(
        password.clone(),
        link.password.clone().unwrap_or_default(),
        state.config.auth.password_hash_iterations,
    )
    .await;

    if !valid {
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
    if let Ok(value) = axum::http::HeaderValue::from_str(&upload_link_cookie(&token)) {
        resp.headers_mut()
            .append(axum::http::header::SET_COOKIE, value);
    }
    Ok(resp)
}
