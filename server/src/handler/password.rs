use axum::{
    Json,
    extract::{Path, Query, State},
};
use serde::Deserialize;
use std::collections::HashMap;
use std::sync::Arc;

use crate::AppState;
use crate::handler::ok_json;
use crate::middleware::auth::AuthUser;
use crate::middleware::repo_extractor::RepoPathRead;
use crate::service::repo::password::PasswordService;
use base::error::AppError;

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
    path: RepoPathRead,
    State(state): State<Arc<AppState>>,
    Json(body): Json<SetPasswordRequest>,
) -> Result<Json<serde_json::Value>, AppError> {
    let repo_id = &path.repo_id;
    let password = body
        .password
        .ok_or_else(|| AppError::BadRequest("password required".into()))?;

    PasswordService::set_password(
        &state.password_manager,
        &state.repos,
        repo_id,
        path.user.user_id,
        &password,
    )
    .await?;

    Ok(ok_json())
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
            // Changing the repo password re-encrypts content for all members →
            // only members with write access may do it.
            crate::domain::permission::check_repo_write_permission(
                state.repos.member.as_ref(),
                &repo_id,
                auth.user_id,
            )
            .await?;

            let old_password = body
                .old_password
                .ok_or_else(|| AppError::BadRequest("old_password required".into()))?;
            let new_password = body
                .new_password
                .ok_or_else(|| AppError::BadRequest("new_password required".into()))?;

            PasswordService::change_password(
                &state.password_manager,
                &state.repos,
                &repo_id,
                auth.user_id,
                &old_password,
                &new_password,
            )
            .await
            .map(|_| ok_json())
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
