use axum::{
    Json, Router,
    extract::{Form, State},
    http::StatusCode,
};
use sea_orm::{ActiveModelTrait, ColumnTrait, EntityTrait, QueryFilter};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

use crate::AppState;
use crate::auth::middleware::AuthUser;
use crate::entity::{repo, user};
use crate::error::AppError;

#[derive(Serialize)]
pub struct AccountInfo {
    pub email: String,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub nickname: Option<String>,
    #[serde(rename = "id")]
    pub id: i32,
    /// Space used in bytes (sum of owned repo sizes).
    pub usage: i64,
    /// Storage quota in bytes. 0 or -2 means unlimited.
    pub total: i64,
}

#[derive(Deserialize)]
pub struct RegisterForm {
    pub email: String,
    pub password: String,
}

#[derive(Deserialize)]
pub struct UpdateAccountInfo {
    /// New display name / nickname.
    pub name: String,
}

pub fn account_routes() -> Router<Arc<AppState>> {
    Router::new()
        .route(
            "/info/",
            axum::routing::get(get_account_info).put(update_account_info),
        )
        .route("/", axum::routing::post(register_user))
}

pub async fn get_account_info(
    auth: AuthUser,
    State(state): State<Arc<AppState>>,
) -> Result<Json<AccountInfo>, AppError> {
    let user = user::Entity::find_by_id(auth.user_id)
        .one(state.db.as_ref())
        .await?
        .ok_or(AppError::Unauthorized)?;

    // Compute usage: sum of sizes for user-owned repos.
    let owned_repos = repo::Entity::find()
        .filter(repo::Column::OwnerId.eq(auth.user_id))
        .all(state.db.as_ref())
        .await?;
    let usage: i64 = owned_repos.iter().map(|r| r.size).sum();

    let total = if state.config.storage.max_storage_bytes > 0 {
        state.config.storage.max_storage_bytes as i64
    } else {
        0 // unlimited
    };

    let nickname = user.nickname();

    Ok(Json(AccountInfo {
        email: user.email.clone(),
        name: nickname.clone(),
        nickname: Some(nickname),
        id: user.id,
        usage,
        total,
    }))
}

/// PUT /api2/account/info/ — update the user's display name/nickname.
///
/// The Seafile client sends `{"name": "new nickname"}`, where `name`
/// is the new display name. We store it in the `display_name` column.
pub async fn update_account_info(
    auth: AuthUser,
    State(state): State<Arc<AppState>>,
    Json(body): Json<UpdateAccountInfo>,
) -> Result<Json<AccountInfo>, AppError> {
    let db = state.db.as_ref();

    let user_record = user::Entity::find_by_id(auth.user_id)
        .one(db)
        .await?
        .ok_or(AppError::Unauthorized)?;

    let mut active: user::ActiveModel = user_record.into();
    active.display_name = sea_orm::Set(if body.name.is_empty() {
        None
    } else {
        Some(body.name.trim().to_string())
    });
    active.update(db).await?;

    // Re-fetch to get fresh data for the response.
    let updated_user = user::Entity::find_by_id(auth.user_id)
        .one(db)
        .await?
        .ok_or(AppError::Unauthorized)?;

    let owned_repos = repo::Entity::find()
        .filter(repo::Column::OwnerId.eq(auth.user_id))
        .all(db)
        .await?;
    let usage: i64 = owned_repos.iter().map(|r| r.size).sum();

    let total = if state.config.storage.max_storage_bytes > 0 {
        state.config.storage.max_storage_bytes as i64
    } else {
        0
    };

    let nickname = updated_user.nickname();

    Ok(Json(AccountInfo {
        email: updated_user.email.clone(),
        name: nickname.clone(),
        nickname: Some(nickname),
        id: updated_user.id,
        usage,
        total,
    }))
}

pub async fn register_user(
    State(state): State<Arc<AppState>>,
    Form(form): Form<RegisterForm>,
) -> Result<StatusCode, AppError> {
    let existing = user::Entity::find()
        .filter(user::Column::Email.eq(&form.email))
        .one(state.db.as_ref())
        .await?;

    if existing.is_some() {
        return Err(AppError::BadRequest("user already exists".into()));
    }

    let iterations = state.config.auth.password_hash_iterations;
    let password_hash = crate::auth::password::hash_password(&form.password, iterations);
    let now = chrono::Utc::now().timestamp();

    let user_model = user::ActiveModel {
        id: sea_orm::NotSet,
        email: sea_orm::Set(form.email),
        password_hash: sea_orm::Set(password_hash),
        is_active: sea_orm::Set(true),
        is_admin: sea_orm::Set(false),
        created_at: sea_orm::Set(now),
        last_login_at: sea_orm::NotSet,
        invited_by: sea_orm::Set(None),
        name: sea_orm::NotSet,
        display_name: sea_orm::NotSet,
    };

    user_model.insert(state.db.as_ref()).await?;

    Ok(StatusCode::OK)
}
