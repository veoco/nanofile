use axum::{
    Json, Router,
    extract::{Path, State},
    routing::{get, put},
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

use crate::AppState;
use crate::auth::middleware::AuthUser;
use crate::error::AppError;

pub fn admin_user_routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/users/", get(list_users).post(create_user))
        .route("/users/{user_id}/", put(update_user).delete(delete_user))
}

/// Check that the authenticated user is an admin.
async fn require_admin(state: &Arc<AppState>, auth: &AuthUser) -> Result<(), AppError> {
    let user_record = state
        .repos
        .user
        .find_by_id(auth.user_id)
        .await?
        .ok_or(AppError::Unauthorized)?;

    if !user_record.is_admin {
        return Err(AppError::Forbidden);
    }
    Ok(())
}

#[derive(Serialize)]
pub struct UserAdminView {
    pub id: i32,
    pub email: String,
    pub is_active: bool,
    pub is_admin: bool,
    pub storage_quota: Option<i64>,
    pub usage: i64,
    pub created_at: i64,
    pub last_login_at: Option<i64>,
}

/// GET /api2/admin/users/ — list all users.
async fn list_users(
    auth: AuthUser,
    State(state): State<Arc<AppState>>,
) -> Result<Json<Vec<UserAdminView>>, AppError> {
    require_admin(&state, &auth).await?;

    let svc = state.admin_user_service();
    let users = svc.list_users().await?;

    Ok(Json(
        users
            .into_iter()
            .map(|u| UserAdminView {
                id: u.id,
                email: u.email,
                is_active: u.is_active,
                is_admin: u.is_admin,
                storage_quota: u.storage_quota,
                usage: u.usage,
                created_at: u.created_at,
                last_login_at: u.last_login_at,
            })
            .collect(),
    ))
}

#[derive(Deserialize)]
pub struct CreateUserPayload {
    pub email: String,
    pub password: String,
    #[serde(default)]
    pub is_admin: bool,
    #[serde(default = "default_true")]
    pub is_active: bool,
    pub storage_quota: Option<i64>,
}

fn default_true() -> bool {
    true
}

/// POST /api2/admin/users/ — create a new user.
async fn create_user(
    auth: AuthUser,
    State(state): State<Arc<AppState>>,
    Json(payload): Json<CreateUserPayload>,
) -> Result<Json<UserAdminView>, AppError> {
    require_admin(&state, &auth).await?;

    let iterations = state.config.auth.password_hash_iterations;
    let password_hash = crate::auth::password::hash_password(&payload.password, iterations);

    let svc = state.admin_user_service();
    svc.create_user(
        payload.email.clone(),
        password_hash,
        payload.is_admin,
        payload.is_active,
        payload.storage_quota,
    )
    .await?;

    // Fetch the newly created user to return full info.
    let created = state
        .repos
        .user
        .find_by_email(&payload.email)
        .await?
        .ok_or_else(|| AppError::Internal("user not found after creation".into()))?;

    let usage = svc.compute_usage(created.id).await?;

    Ok(Json(UserAdminView {
        id: created.id,
        email: created.email,
        is_active: created.is_active,
        is_admin: created.is_admin,
        storage_quota: created.storage_quota,
        usage,
        created_at: created.created_at,
        last_login_at: created.last_login_at,
    }))
}

#[derive(Deserialize)]
pub struct UpdateUserPayload {
    pub is_admin: bool,
    pub is_active: bool,
    pub storage_quota: Option<i64>,
}

/// PUT /api2/admin/users/{user_id}/ — update a user.
async fn update_user(
    auth: AuthUser,
    State(state): State<Arc<AppState>>,
    Path(user_id): Path<i32>,
    Json(payload): Json<UpdateUserPayload>,
) -> Result<Json<UserAdminView>, AppError> {
    require_admin(&state, &auth).await?;

    let svc = state.admin_user_service();
    svc.update_user(
        user_id,
        payload.is_admin,
        payload.is_active,
        payload.storage_quota,
    )
    .await?;

    // Fetch updated user
    let updated = state
        .repos
        .user
        .find_by_id(user_id)
        .await?
        .ok_or_else(|| AppError::NotFound("user not found".into()))?;

    let usage = svc.compute_usage(updated.id).await?;

    Ok(Json(UserAdminView {
        id: updated.id,
        email: updated.email,
        is_active: updated.is_active,
        is_admin: updated.is_admin,
        storage_quota: updated.storage_quota,
        usage,
        created_at: updated.created_at,
        last_login_at: updated.last_login_at,
    }))
}

/// DELETE /api2/admin/users/{user_id}/ — delete a user.
async fn delete_user(
    auth: AuthUser,
    State(state): State<Arc<AppState>>,
    Path(user_id): Path<i32>,
) -> Result<Json<serde_json::Value>, AppError> {
    require_admin(&state, &auth).await?;

    let svc = state.admin_user_service();
    svc.delete_user(user_id).await?;

    Ok(Json(serde_json::json!({"success": true})))
}
