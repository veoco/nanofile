use askama::Template;
use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::{Html, IntoResponse, Response},
};
use std::collections::HashMap;
use std::sync::Arc;

use crate::AppState;
use base::error::AppError;

// ── Templates ─────────────────────────────────────────────────────────────

#[allow(dead_code)]
#[derive(Template)]
#[template(path = "web/upload_link_view.html")]
struct UploadLinkViewTemplate {
    pub token: String,
    pub repo_id: String,
    pub path: String,
    pub dir_name: String,
    pub password_query: String,
    pub has_password: bool,
    pub max_upload_size_mb: i64,
    pub description: Option<String>,
}

#[allow(dead_code)]
#[derive(Template)]
#[template(path = "web/share_access_validation.html")]
struct ShareAccessValidationTemplate {
    pub token: String,
    pub error: Option<String>,
    pub form_action: String,
}

// ── Helpers ───────────────────────────────────────────────────────────────

/// Simple URL encoding for password (same as share_view.rs).
fn urlencoding(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char);
            }
            b' ' => out.push_str("%20"),
            _ => {
                out.push_str(&format!("%{:02X}", b));
            }
        }
    }
    out
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
    let repo_exists = state.repos.repo.find_by_id(&link.repo_id).await?.is_some();
    if !repo_exists {
        return Err(AppError::NotFound("Upload link not found".into()));
    }

    Ok(link)
}

/// Check whether the password in the request matches the stored hash.
fn check_password(
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
            crate::auth::password::verify_password(pwd, stored_hash, password_hash_iterations)
        }
        None => false,
    }
}

// ── Main GET handler ──────────────────────────────────────────────────────

/// GET /u/{token}/ — show the public upload page.
pub async fn upload_link_view(
    State(state): State<Arc<AppState>>,
    Path(token): Path<String>,
    Query(params): Query<HashMap<String, String>>,
) -> Result<Response, AppError> {
    let link = validate_upload_link(&state, &token).await?;

    // Password check
    let pw_ok = check_password(&link, &params, state.config.auth.password_hash_iterations);

    // If password is required but not provided, show password form
    if link.password.is_some() && !pw_ok {
        let error = if params.contains_key("password") {
            Some("Incorrect password".to_string())
        } else {
            None
        };
        let tpl = ShareAccessValidationTemplate {
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

    let password_query = if let Some(pwd) = params.get("password") {
        format!("?password={}", urlencoding(pwd))
    } else {
        String::new()
    };

    let tpl = UploadLinkViewTemplate {
        token: link.token.clone(),
        repo_id: link.repo_id.clone(),
        path: link.path.clone(),
        dir_name,
        password_query,
        has_password: link.password.is_some(),
        max_upload_size_mb: state.config.server.max_upload_size_mb as i64,
        description: link.description.clone(),
    };

    let html = tpl
        .render()
        .map_err(|e| AppError::Internal(e.to_string()))?;
    Ok(Html(html).into_response())
}

// ── POST handler for password submission ──────────────────────────────────

/// POST /u/{token}/ — validate password, redirect with password in URL.
pub async fn upload_link_view_post(
    State(state): State<Arc<AppState>>,
    Path(token): Path<String>,
    axum::Form(form): axum::Form<HashMap<String, String>>,
) -> Result<Response, AppError> {
    let link = validate_upload_link(&state, &token).await?;

    let password = form
        .get("password")
        .ok_or_else(|| AppError::BadRequest("password required".into()))?;

    let valid = crate::auth::password::verify_password(
        password,
        &link.password.unwrap_or_default(),
        state.config.auth.password_hash_iterations,
    );

    if !valid {
        let tpl = ShareAccessValidationTemplate {
            token: token.clone(),
            error: Some("Incorrect password".to_string()),
            form_action: format!("/u/{}/", token),
        };
        let html = tpl
            .render()
            .map_err(|e| AppError::Internal(e.to_string()))?;
        return Ok(Html(html).into_response());
    }

    let redirect = format!("/u/{}/?password={}", token, urlencoding(password));
    Ok((StatusCode::FOUND, [("Location", redirect.as_str())]).into_response())
}
