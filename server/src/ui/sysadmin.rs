/// Web UI admin page — user management.
use askama::Template;
use axum::{
    Form,
    extract::{Path, State},
    http::StatusCode,
    response::{Html, IntoResponse, Redirect, Response},
};
use serde::Deserialize;
use std::sync::Arc;

use crate::AppState;
use crate::admin::service::AdminUserService;
use crate::error::AppError;

use super::auth_extractor::WebUser;

#[derive(Template)]
#[template(path = "sysadmin/index.html")]
pub struct SysAdminTemplate {
    pub urls: &'static crate::static_assets::TemplateUrls,
    pub user_email: String,
    pub is_admin: bool,
    pub csrf_token: Option<String>,
    pub active_page: &'static str,
    pub users: Vec<UserRow>,
    pub error: Option<String>,
    pub success: Option<String>,
    pub left_panel_repos: Vec<crate::repo::LeftPanelRepo>,
    pub current_repo_id: Option<String>,
}

pub struct UserRow {
    pub id: i32,
    pub email: String,
    pub is_active: bool,
    pub is_admin: bool,
    pub storage_quota_display: String,
    pub usage_formatted: String,
    pub quota_formatted: String,
    pub created_at: String,
    pub last_login_at: String,
}

/// GET /sysadmin/users/ — user management page (admin only).
pub async fn sysadmin_page(user: WebUser, State(state): State<Arc<AppState>>) -> Response {
    if !user.is_admin {
        return Redirect::to("/libraries/").into_response();
    }

    let db = state.db.as_ref();
    let svc = AdminUserService::new(db, &state.repos);
    let users_data = match svc.list_users().await {
        Ok(u) => u,
        Err(e) => return AppError::internal(e.to_string()).into_response(),
    };

    let csrf_token = Some(crate::auth::csrf::generate_csrf_token(
        &state.csrf_secret,
        &user.session_token,
    ));

    let left_panel_repos = match crate::repo::load_left_panel_repos(db, user.user_id).await {
        Ok(r) => r,
        Err(e) => return AppError::internal(e.to_string()).into_response(),
    };

    let users: Vec<UserRow> = users_data
        .into_iter()
        .map(|u| {
            let quota_display = match u.storage_quota {
                Some(0) => "Unlimited".to_string(),
                Some(q) => crate::ui::files::format_size(q),
                None => format!(
                    "{} (global)",
                    crate::ui::files::format_size(state.config.storage.max_storage_bytes as i64)
                ),
            };
            UserRow {
                id: u.id,
                email: u.email,
                is_active: u.is_active,
                is_admin: u.is_admin,
                storage_quota_display: u.storage_quota.map(|q| q.to_string()).unwrap_or_default(),
                usage_formatted: crate::ui::files::format_size(u.usage),
                quota_formatted: quota_display,
                created_at: format_ts(u.created_at),
                last_login_at: u
                    .last_login_at
                    .map(format_ts)
                    .unwrap_or_else(|| "Never".to_string()),
            }
        })
        .collect();

    let tpl = SysAdminTemplate {
        urls: crate::static_assets::template_urls(),
        user_email: user.email,
        is_admin: user.is_admin,
        csrf_token,
        active_page: "sysadmin",
        users,
        error: None,
        success: None,
        left_panel_repos,
        current_repo_id: None,
    };

    match tpl.render() {
        Ok(html) => Html(html).into_response(),
        Err(e) => AppError::internal(e.to_string()).into_response(),
    }
}

fn format_ts(ts: i64) -> String {
    use chrono::{DateTime, Utc};
    let dt: DateTime<Utc> = DateTime::from_timestamp(ts, 0).unwrap_or_default();
    dt.format("%Y-%m-%d %H:%M").to_string()
}

// ─── POST handlers ────────────────────────────────────────────────────────

#[derive(Deserialize)]
pub struct CreateUserForm {
    pub email: String,
    pub password: String,
    pub is_admin: Option<String>,      // "on" or absent
    pub is_active: Option<String>,     // "on" or absent (default on)
    pub storage_quota: Option<String>, // empty = use global, "0" = unlimited
    pub csrf_token: Option<String>,
}

/// POST /sysadmin/users/create/ — create a new user.
pub async fn create_user(
    user: WebUser,
    State(state): State<Arc<AppState>>,
    Form(form): Form<CreateUserForm>,
) -> Result<impl IntoResponse, AppError> {
    if !user.is_admin {
        return Err(AppError::Forbidden);
    }

    // CSRF check
    if let Some(ref token) = form.csrf_token {
        let expected =
            crate::auth::csrf::generate_csrf_token(&state.csrf_secret, &user.session_token);
        if *token != expected {
            return Err(AppError::BadRequest("Invalid CSRF token.".to_string()));
        }
    }

    let svc = AdminUserService::new(state.db.as_ref(), &state.repos);

    let storage_quota = parse_quota(form.storage_quota.as_deref());
    let is_admin = form.is_admin.is_some();
    let is_active = form.is_active.is_some();

    let iterations = state.config.auth.password_hash_iterations;
    let password_hash = crate::auth::password::hash_password(&form.password, iterations);

    if let Err(e) = svc
        .create_user(
            form.email,
            password_hash,
            is_admin,
            is_active,
            storage_quota,
        )
        .await
    {
        return Ok(render_sysadmin_error(&state, e).await);
    }

    Ok((StatusCode::FOUND, [("Location", "/sysadmin/users/")]).into_response())
}

#[derive(Deserialize)]
pub struct UpdateUserForm {
    pub is_admin: Option<String>,
    pub is_active: Option<String>,
    pub storage_quota: Option<String>,
    pub csrf_token: Option<String>,
}

/// POST /sysadmin/users/{user_id}/update/ — update a user.
pub async fn update_user(
    user: WebUser,
    State(state): State<Arc<AppState>>,
    Path(user_id): Path<i32>,
    Form(form): Form<UpdateUserForm>,
) -> Result<impl IntoResponse, AppError> {
    if !user.is_admin {
        return Err(AppError::Forbidden);
    }

    // CSRF check
    if let Some(ref token) = form.csrf_token {
        let expected =
            crate::auth::csrf::generate_csrf_token(&state.csrf_secret, &user.session_token);
        if *token != expected {
            return Err(AppError::BadRequest("Invalid CSRF token.".to_string()));
        }
    }

    let svc = AdminUserService::new(state.db.as_ref(), &state.repos);

    let storage_quota = parse_quota(form.storage_quota.as_deref());
    let is_admin = form.is_admin.is_some();
    let is_active = form.is_active.is_some();

    if let Err(e) = svc
        .update_user(user_id, is_admin, is_active, storage_quota)
        .await
    {
        return Ok(render_sysadmin_error(&state, e).await);
    }

    Ok((StatusCode::FOUND, [("Location", "/sysadmin/users/")]).into_response())
}

/// POST /sysadmin/users/{user_id}/delete/ — delete a user.
pub async fn delete_user(
    user: WebUser,
    State(state): State<Arc<AppState>>,
    Path(user_id): Path<i32>,
) -> Result<impl IntoResponse, AppError> {
    if !user.is_admin {
        return Err(AppError::Forbidden);
    }

    let svc = AdminUserService::new(state.db.as_ref(), &state.repos);

    if let Err(e) = svc.delete_user(user_id).await {
        return Ok(render_sysadmin_error(&state, e).await);
    }

    Ok((StatusCode::FOUND, [("Location", "/sysadmin/users/")]).into_response())
}

/// Parse an optional storage quota string.
/// Empty string → None (use global default)
/// "0" → Some(0) (unlimited)
/// Other numeric → Some(parsed)
fn parse_quota(s: Option<&str>) -> Option<i64> {
    match s {
        None | Some("") => None,
        Some(q) => q.parse::<i64>().ok(),
    }
}

/// Re-render the sysadmin page with an error message.
async fn render_sysadmin_error(_state: &Arc<AppState>, _error: AppError) -> Response {
    // For simplicity, just redirect back — we can't extract WebUser here to
    // re-render the template with an error message inline.
    (StatusCode::FOUND, [("Location", "/sysadmin/users/")]).into_response()
}
