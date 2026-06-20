use axum::{
    Json,
    extract::{Path, Query, State},
};
use serde::Deserialize;
use std::collections::HashMap;
use std::sync::Arc;

use crate::AppState;
use crate::auth::middleware::AuthUser;
use crate::error::AppError;
use crate::repo::service::password::PasswordService;

/// Request body for setting a repo password.
#[derive(Deserialize)]
pub struct SetPasswordRequest {
    pub password: Option<String>,
}

/// Request body for changing a repo password.
#[derive(Deserialize)]
pub struct ChangePasswordRequest {
    pub old_password: Option<String>,
    pub new_password: Option<String>,
}

/// POST /api/v2.1/repos/{repo_id}/set-password/
///
/// Set the password for an encrypted repo.
pub async fn set_password_v21(
    auth: AuthUser,
    State(state): State<Arc<AppState>>,
    Path(repo_id): Path<String>,
    Json(body): Json<SetPasswordRequest>,
) -> Result<Json<serde_json::Value>, AppError> {
    let password = body
        .password
        .ok_or_else(|| AppError::BadRequest("password required".into()))?;

    PasswordService::set_password(
        &state.password_manager,
        &state.repos,
        &repo_id,
        auth.user_id,
        &password,
    )
    .await?;

    Ok(Json(serde_json::json!({"success": true})))
}

/// PUT /api/v2.1/repos/{repo_id}/set-password/?operation=change-password
///
/// Change an encrypted repo's password.
pub async fn change_password_v21(
    auth: AuthUser,
    State(state): State<Arc<AppState>>,
    Path(repo_id): Path<String>,
    Query(params): Query<HashMap<String, String>>,
    Json(body): Json<ChangePasswordRequest>,
) -> Result<Json<serde_json::Value>, AppError> {
    let operation = params.get("operation").map(|s| s.as_str());

    match operation {
        Some("change-password") => {
            let old_password = body
                .old_password
                .ok_or_else(|| AppError::BadRequest("old_password required".into()))?;
            let new_password = body
                .new_password
                .ok_or_else(|| AppError::BadRequest("new_password required".into()))?;

            PasswordService::change_password(
                &state.password_manager,
                &state.repos,
                state.db.as_ref(),
                &repo_id,
                auth.user_id,
                &old_password,
                &new_password,
            )
            .await
            .map(|_| Json(serde_json::json!({"success": true})))
        }
        Some("check-password") => {
            let is_set = state
                .password_manager
                .is_password_set(&repo_id, auth.user_id)
                .await;
            Ok(Json(serde_json::json!({"is_set": is_set})))
        }
        _ => Err(AppError::BadRequest(
            "unknown operation; use change-password or check-password".into(),
        )),
    }
}
