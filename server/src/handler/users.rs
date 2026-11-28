//! Administrator user management: the seahub `/api/v2.1/admin/users/` API.
//!
//! The route segment is the account's email (seahub matches `(?P<email>[^/]+@[^/]+)`
//! even though the reference names the parameter `{user_id}`). The list answer is
//! seahub's `{"data": [...], "total_count": n}` envelope; the item fields follow
//! `get_user_info()` with the parts Nanofile stores (no login id, no contact-email
//! profile, no social auth, and `role` is always `default`).

use axum::Router;
use axum::{
    Json,
    extract::{Path, Query, State},
    routing::get,
};
use serde::Deserialize;
use serde_json::{Value, json};
use std::sync::Arc;

use crate::AppState;
use crate::handler::json_or_form::JsonOrForm;
use crate::middleware::auth::AuthUser;
use crate::service::admin::UserAdminInfo;
use crate::service::user::{effective_quota_total, primary_avatar_url};
use base::error::AppError;

pub fn admin_user_routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/users/", get(list_users).post(create_user))
        .route(
            "/users/{user_id}/",
            get(get_user).put(update_user).delete(delete_user),
        )
}

/// seahub's list query parameters. `role` and `source` are accepted for shape
/// compatibility; Nanofile has no roles and no LDAP, so a non-default role
/// filters everything out and `ldapimport` answers an empty page.
#[derive(Deserialize)]
pub struct UserListQuery {
    page: Option<i64>,
    per_page: Option<i64>,
    is_active: Option<String>,
    role: Option<String>,
    source: Option<String>,
    order_by: Option<String>,
    direction: Option<String>,
}

fn get_str(v: &Value, key: &str) -> Option<String> {
    v.get(key).and_then(Value::as_str).map(str::to_string)
}

/// Parse a boolean that may arrive as a JSON bool or as a form string.
fn get_bool(v: &Value, key: &str, label: &str) -> Result<Option<bool>, AppError> {
    match v.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Bool(b)) => Ok(Some(*b)),
        Some(Value::String(s)) => infra::settings::parse_bool(s)
            .map(Some)
            .map_err(|_| AppError::BadRequest(format!("{label} invalid."))),
        _ => Err(AppError::BadRequest(format!("{label} invalid."))),
    }
}

/// A numeric field that may arrive as a JSON number or as a form string.
fn get_i64(v: &Value, key: &str, label: &str) -> Result<Option<i64>, AppError> {
    match v.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Number(n)) => n
            .as_i64()
            .map(Some)
            .ok_or_else(|| AppError::BadRequest(format!("{label} invalid."))),
        Some(Value::String(s)) => s
            .trim()
            .parse::<i64>()
            .map(Some)
            .map_err(|_| AppError::BadRequest(format!("{label} invalid."))),
        _ => Err(AppError::BadRequest(format!("{label} invalid."))),
    }
}

fn account_item(
    email: &str,
    name: &str,
    is_staff: bool,
    is_active: bool,
    storage_quota: Option<i64>,
    created_at: i64,
    last_login_at: Option<i64>,
    usage: i64,
    default_quota: u64,
    site_origin: &str,
) -> Value {
    let last_login = last_login_at
        .map(infra::common::util::timestamp_rfc3339)
        .unwrap_or_default();
    json!({
        "avatar_url": format!("{site_origin}{}", primary_avatar_url(email, 80)),
        "email": email,
        "name": name,
        "contact_email": email,
        "login_id": "",
        "is_staff": is_staff,
        "is_active": is_active,
        "quota_usage": usage,
        "quota_total": effective_quota_total(storage_quota, default_quota),
        "role": "default",
        "create_time": infra::common::util::timestamp_rfc3339(created_at),
        "last_login": last_login,
        "last_access_time": last_login,
        "social_auth": [],
    })
}

fn item_from_model(state: &AppState, user: &infra::entity::user::Model, usage: i64) -> Value {
    let config = state.config();
    account_item(
        &user.email,
        &user.nickname(),
        user.is_admin,
        user.is_active,
        user.storage_quota,
        user.created_at,
        user.last_login_at,
        usage,
        config.storage.default_user_quota_bytes,
        &config.server.site_url_origin(),
    )
}

fn item_from_admin_info(state: &AppState, user: &UserAdminInfo) -> Value {
    let config = state.config();
    account_item(
        &user.email,
        &user.name,
        user.is_admin,
        user.is_active,
        user.storage_quota,
        user.created_at,
        user.last_login_at,
        user.usage,
        config.storage.default_user_quota_bytes,
        &config.server.site_url_origin(),
    )
}

/// `GET /api/v2.1/admin/users/` — list all accounts in seahub's envelope.
pub async fn list_users(
    auth: AuthUser,
    State(state): State<Arc<AppState>>,
    Query(query): Query<UserListQuery>,
) -> Result<Json<Value>, AppError> {
    auth.require_admin(&state).await?;

    let page = query.page.filter(|p| *p > 0).unwrap_or(1);
    let per_page = query
        .per_page
        .filter(|p| *p > 0)
        .map(|p| p.min(500))
        .unwrap_or(25);
    let source = query
        .source
        .map(|s| s.trim().to_ascii_lowercase())
        .unwrap_or_else(|| "db".to_string());
    if source != "db" && source != "ldapimport" {
        return Err(AppError::BadRequest(format!("source {source} invalid.")));
    }
    let order_by = query
        .order_by
        .map(|s| s.trim().to_ascii_lowercase())
        .unwrap_or_default();
    if !order_by.is_empty() && order_by != "quota_usage" {
        return Err(AppError::BadRequest("order_by invalid.".into()));
    }
    let direction = query
        .direction
        .map(|s| s.trim().to_ascii_lowercase())
        .unwrap_or_else(|| "desc".to_string());
    if direction != "asc" && direction != "desc" {
        return Err(AppError::BadRequest("direction invalid.".into()));
    }

    // No LDAP backend: `source=ldapimport` is a valid seahub value that answers
    // an empty page here.
    if source == "ldapimport" {
        return Ok(Json(json!({"data": [], "total_count": 0})));
    }

    let mut users = state.admin_user_service().list_users().await?;
    let total_count = users.len();

    if let Some(is_active) = query.is_active.as_deref() {
        match is_active {
            "1" => users.retain(|u| u.is_active),
            "0" => users.retain(|u| !u.is_active),
            _ => {}
        }
    }
    // Nanofile has one role, so any other value matches no account.
    if let Some(role) = query.role.as_deref()
        && !role.is_empty()
        && role != "default"
    {
        users.clear();
    }

    if !order_by.is_empty() {
        if total_count > 500 {
            return Err(AppError::BadRequest(
                "There are more than 500 users, and sort is not offered.".into(),
            ));
        }
        users.sort_by_key(|u| u.usage);
        if direction != "asc" {
            users.reverse();
        }
    }

    let start = ((page - 1) as usize).saturating_mul(per_page as usize);
    let data: Vec<Value> = users
        .into_iter()
        .skip(start)
        .take(per_page as usize)
        .map(|u| item_from_admin_info(&state, &u))
        .collect();
    Ok(Json(json!({"data": data, "total_count": total_count})))
}

/// `GET /api/v2.1/admin/users/{user_id}/` — one account; the segment is the email.
pub async fn get_user(
    auth: AuthUser,
    State(state): State<Arc<AppState>>,
    Path(email): Path<String>,
) -> Result<Json<Value>, AppError> {
    auth.require_admin(&state).await?;
    let user = state
        .repos
        .user
        .find_by_email(&email)
        .await?
        .ok_or_else(|| AppError::NotFound(format!("User {email} not found.")))?;
    let usage = state.admin_user_service().compute_usage(user.id).await?;
    Ok(Json(item_from_model(&state, &user, usage)))
}

/// `POST /api/v2.1/admin/users/` — create an account.
pub async fn create_user(
    auth: AuthUser,
    State(state): State<Arc<AppState>>,
    JsonOrForm(body): JsonOrForm,
) -> Result<Json<Value>, AppError> {
    auth.require_admin(&state).await?;

    let email = match get_str(&body, "email") {
        Some(email) if crate::service::mail::settings::is_valid_address(&email) => email,
        _ => return Err(AppError::BadRequest("email invalid.".into())),
    };
    if state.repos.user.exists_by_email(&email).await? {
        return Err(AppError::BadRequest(format!(
            "User {email} already exists."
        )));
    }
    let password = get_str(&body, "password")
        .ok_or_else(|| AppError::BadRequest("password required.".into()))?;
    let config = state.config();
    crate::service::auth::password::validate_password(
        &password,
        config.auth.password_min_length,
        config.auth.require_strong_password,
    )
    .map_err(AppError::BadRequest)?;

    let is_admin = get_bool(&body, "is_staff", "is_staff")?.unwrap_or(false);
    let is_active = get_bool(&body, "is_active", "is_active")?.unwrap_or(true);
    let name = validate_name(get_str(&body, "name"))?;
    let quota_total = get_i64(&body, "quota_total", "quota_total")?;
    let storage_quota = quota_total_to_bytes(quota_total)?;
    validate_role(get_str(&body, "role"))?;

    let password_hash = crate::service::auth::password::hash_password_async(
        password,
        config.auth.password_hash_iterations,
    )
    .await;
    state
        .admin_user_service()
        .create_user(
            email.clone(),
            password_hash,
            is_admin,
            is_active,
            storage_quota,
        )
        .await?;

    let created = state
        .repos
        .user
        .find_by_email(&email)
        .await?
        .ok_or_else(|| AppError::Internal("user not found after creation".into()))?;
    if let Some(name) = name {
        state
            .repos
            .user
            .update_display_name(created.id, name)
            .await?;
    }
    let created = state
        .repos
        .user
        .find_by_email(&email)
        .await?
        .ok_or_else(|| AppError::Internal("user not found after creation".into()))?;
    let usage = state.admin_user_service().compute_usage(created.id).await?;
    Ok(Json(item_from_model(&state, &created, usage)))
}

/// `PUT /api/v2.1/admin/users/{user_id}/` — partial update, seahub style.
pub async fn update_user(
    auth: AuthUser,
    State(state): State<Arc<AppState>>,
    Path(email): Path<String>,
    JsonOrForm(body): JsonOrForm,
) -> Result<Json<Value>, AppError> {
    auth.require_admin(&state).await?;

    let existing = state
        .repos
        .user
        .find_by_email(&email)
        .await?
        .ok_or_else(|| AppError::NotFound(format!("User {email} not found.")))?;

    let is_admin = get_bool(&body, "is_staff", "is_staff")?.unwrap_or(existing.is_admin);
    let is_active = get_bool(&body, "is_active", "is_active")?.unwrap_or(existing.is_active);
    let quota_total = get_i64(&body, "quota_total", "quota_total")?;
    let storage_quota = match quota_total {
        Some(_) => quota_total_to_bytes(quota_total)?,
        None => existing.storage_quota,
    };
    let name = validate_name(get_str(&body, "name"))?;
    let password = get_str(&body, "password");
    validate_role(get_str(&body, "role"))?;

    state
        .admin_user_service()
        .update_user(existing.id, is_admin, is_active, storage_quota)
        .await?;
    if let Some(name) = name {
        state
            .repos
            .user
            .update_display_name(existing.id, name)
            .await?;
    }
    if let Some(password) = password {
        let config = state.config();
        crate::service::auth::password::validate_password(
            &password,
            config.auth.password_min_length,
            config.auth.require_strong_password,
        )
        .map_err(AppError::BadRequest)?;
        let hash = crate::service::auth::password::hash_password_async(
            password,
            config.auth.password_hash_iterations,
        )
        .await;
        state.repos.user.update_password(existing.id, hash).await?;
    }

    let updated = state
        .repos
        .user
        .find_by_email(&email)
        .await?
        .ok_or_else(|| AppError::NotFound(format!("User {email} not found.")))?;
    let usage = state.admin_user_service().compute_usage(updated.id).await?;
    let mut item = item_from_model(&state, &updated, usage);
    item["update_status_tip"] = Value::String(String::new());
    Ok(Json(item))
}

/// `DELETE /api/v2.1/admin/users/{user_id}/` — delete an account.
pub async fn delete_user(
    auth: AuthUser,
    State(state): State<Arc<AppState>>,
    Path(email): Path<String>,
) -> Result<Json<Value>, AppError> {
    auth.require_admin(&state).await?;
    let user = state
        .repos
        .user
        .find_by_email(&email)
        .await?
        .ok_or_else(|| AppError::NotFound(format!("User {email} not found.")))?;
    state.admin_user_service().delete_user(user.id).await?;
    Ok(Json(json!({"success": true})))
}

/// seahub accepts a nickname of at most 64 characters without `/`; an empty
/// value clears it.
fn validate_name(name: Option<String>) -> Result<Option<Option<String>>, AppError> {
    let Some(name) = name else {
        return Ok(None);
    };
    let name = name.trim().to_string();
    if name.len() > 64 {
        return Err(AppError::BadRequest(
            "Name is too long (maximum is 64 characters).".into(),
        ));
    }
    if name.contains('/') {
        return Err(AppError::BadRequest("Name should not include '/'.".into()));
    }
    Ok(Some(if name.is_empty() { None } else { Some(name) }))
}

/// `quota_total` arrives in MB (seahub's unit) and is stored in bytes. `0` means
/// unlimited, matching seahub's "0 = default/unlimited" reading.
fn quota_total_to_bytes(mb: Option<i64>) -> Result<Option<i64>, AppError> {
    let Some(mb) = mb else {
        return Ok(None);
    };
    if mb < 0 {
        return Err(AppError::BadRequest(
            "Space quota is too low (minimum value is 0).".into(),
        ));
    }
    Ok(Some(mb.saturating_mul(1024 * 1024)))
}

/// Nanofile has no custom roles: only an empty value or `default` is accepted.
fn validate_role(role: Option<String>) -> Result<(), AppError> {
    match role.as_deref() {
        None | Some("") | Some("default") => Ok(()),
        Some(_) => Err(AppError::BadRequest("role must be in ['default'].".into())),
    }
}
