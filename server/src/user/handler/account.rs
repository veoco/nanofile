use axum::{
    Json, Router,
    extract::{Form, State},
    http::StatusCode,
};
use serde::Deserialize;
use std::sync::Arc;

use crate::AppState;
use crate::auth::middleware::AuthUser;
use crate::user::service::{AccountInfo, AccountService};
use base::error::AppError;

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
    let svc = AccountService::new(state.repos.clone());
    let info = svc
        .get_account_info(auth.user_id, state.config.storage.max_storage_bytes)
        .await?;
    Ok(Json(info))
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
    let svc = AccountService::new(state.repos.clone());
    let info = svc
        .update_account_info(
            auth.user_id,
            body.name,
            state.config.storage.max_storage_bytes,
        )
        .await?;
    Ok(Json(info))
}

pub async fn register_user(
    State(state): State<Arc<AppState>>,
    Form(form): Form<RegisterForm>,
) -> Result<StatusCode, AppError> {
    let iterations = state.config.auth.password_hash_iterations;
    let password_hash = crate::auth::password::hash_password(&form.password, iterations);

    let svc = AccountService::new(state.repos.clone());
    svc.register_user(form.email, password_hash).await?;

    Ok(StatusCode::OK)
}
