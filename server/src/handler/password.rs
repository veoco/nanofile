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
///
/// This is the endpoint the Android client uses before downloading from an
/// encrypted library, so it verifies the same password as
/// `POST /api2/repos/{id}/?op=setpassword` and must share that endpoint's
/// per-(user, repo) failed-attempt meter. Skipping it here would leave an
/// unmetered oracle against a KDF whose iteration count is fixed at 1000 by the
/// Seafile wire protocol.
pub async fn set_password_v21(
    path: RepoPathRead,
    State(state): State<Arc<AppState>>,
    Json(body): Json<SetPasswordRequest>,
) -> Result<Json<serde_json::Value>, AppError> {
    let repo_id = &path.repo_id;
    let user_id = path.user.user_id;

    if state
        .auth_limiters
        .is_repo_password_limited(user_id, repo_id)
    {
        return Err(AppError::TooManyRequests);
    }

    let password = body
        .password
        .ok_or_else(|| AppError::BadRequest("password required".into()))?;

    if let Err(e) = PasswordService::set_password(
        &state.password_manager,
        &state.repos,
        repo_id,
        user_id,
        &password,
    )
    .await
    {
        // Only a wrong password is a guess. Client retries with their own
        // correct password are cleared below and never consume budget.
        if matches!(e, base::error::AppError::RepoPasswdRequired) {
            state
                .auth_limiters
                .record_repo_password_failure(user_id, repo_id);
        }
        return Err(e);
    }

    state
        .auth_limiters
        .clear_repo_password_failures(user_id, repo_id);

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
            // Rotating the library password rewrites the repo-wide `magic` and
            // `random_key`, which locks out every other member if they only
            // knew the old password. It is a library-wide cryptographic
            // operation, so it is owner-only — matching how member management
            // is already gated elsewhere.
            crate::domain::permission::check_repo_owner(
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

            // The old password is verified against the stored magic, so this is
            // a fourth verification path and shares the same per-(user, repo)
            // meter (a successful rotation clears it).
            if state
                .auth_limiters
                .is_repo_password_limited(auth.user_id, &repo_id)
            {
                return Err(AppError::TooManyRequests);
            }

            match PasswordService::change_password(
                &state.password_manager,
                &state.repos,
                &repo_id,
                auth.user_id,
                &old_password,
                &new_password,
            )
            .await
            {
                Ok(()) => {
                    state
                        .auth_limiters
                        .clear_repo_password_failures(auth.user_id, &repo_id);
                    Ok(ok_json())
                }
                Err(e) => {
                    if matches!(e, base::error::AppError::RepoPasswdRequired) {
                        state
                            .auth_limiters
                            .record_repo_password_failure(auth.user_id, &repo_id);
                    }
                    Err(e)
                }
            }
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
