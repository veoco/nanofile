use axum::{Json, Router, extract::State};
use serde::Deserialize;
use std::sync::Arc;

use crate::AppState;
use crate::middleware::auth::AuthUser;
use crate::service::user::{AccountInfo, AccountService};
use base::error::AppError;

#[derive(Deserialize)]
pub struct UpdateAccountInfo {
    pub name: String,
}

pub fn account_routes() -> Router<Arc<AppState>> {
    Router::new().route(
        "/info/",
        axum::routing::get(get_account_info).put(update_account_info),
    )
}

pub async fn get_account_info(
    auth: AuthUser,
    State(state): State<Arc<AppState>>,
) -> Result<Json<AccountInfo>, AppError> {
    let svc = AccountService::new(state.repos.clone());
    let info = svc
        .get_account_info(
            auth.user_id,
            state.config().storage.default_user_quota_bytes,
            &state.config().server.site_url_origin(),
        )
        .await?;
    Ok(Json(info))
}

/// PUT /api2/account/info/ — the Seafile client sends `{"name": "new nickname"}`;
/// it is stored in the `display_name` column.
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
            state.config().storage.default_user_quota_bytes,
            &state.config().server.site_url_origin(),
        )
        .await?;
    Ok(Json(info))
}
