//! Extractors that combine authentication with repo permission checking.
//!
//! Each one authenticates the user (`AuthUser`), takes `repo_id` from the path,
//! then checks read or write permission on that repo.

use axum::extract::{FromRequestParts, Path};
use axum::http::request::Parts;
use std::sync::Arc;

use crate::AppState;
use crate::middleware::auth::{AuthRejection, AuthUser};
use axum::http::StatusCode;
use base::error::AppError;

fn auth_rejection_to_app_error(rejection: AuthRejection) -> AppError {
    match rejection.status() {
        StatusCode::FORBIDDEN => AppError::Forbidden,
        _ => AppError::Unauthorized,
    }
}

/// Authenticated user plus a checked repo read permission.
#[derive(Debug, Clone)]
pub struct RepoPathRead {
    pub user: AuthUser,
    pub repo_id: String,
}

/// Authenticated user plus a checked repo write permission.
#[derive(Debug, Clone)]
pub struct RepoPathWrite {
    pub user: AuthUser,
    pub repo_id: String,
}

impl FromRequestParts<Arc<AppState>> for RepoPathRead {
    type Rejection = AppError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &Arc<AppState>,
    ) -> Result<Self, Self::Rejection> {
        // The status is preserved: a missing or invalid credential is 401, while a
        // key lacking the capability or library scope is 403 — collapsing both into
        // 401 would ask the client to re-authenticate when that cannot help.
        let user = AuthUser::from_request_parts(parts, state)
            .await
            .map_err(auth_rejection_to_app_error)?;

        let Path(repo_id) = Path::<String>::from_request_parts(parts, state)
            .await
            .map_err(|_| AppError::BadRequest("missing repo_id in path".into()))?;

        crate::domain::permission::check_repo_read_permission(
            state.repos.member.as_ref(),
            &repo_id,
            user.user_id,
        )
        .await?;

        // A unified API key adds a library-scope ceiling on top of membership:
        // the key must be bound to this library (or bound to all of them).
        if !user.credential.allows_repo(&repo_id, false) {
            return Err(AppError::Forbidden);
        }

        Ok(RepoPathRead { user, repo_id })
    }
}

impl FromRequestParts<Arc<AppState>> for RepoPathWrite {
    type Rejection = AppError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &Arc<AppState>,
    ) -> Result<Self, Self::Rejection> {
        // Same 401/403 distinction: a key lacking the capability or scope is 403,
        // and asking it to re-authenticate would not help.
        let user = AuthUser::from_request_parts(parts, state)
            .await
            .map_err(auth_rejection_to_app_error)?;

        let Path(repo_id) = Path::<String>::from_request_parts(parts, state)
            .await
            .map_err(|_| AppError::BadRequest("missing repo_id in path".into()))?;

        crate::domain::permission::check_repo_write_permission(
            state.repos.member.as_ref(),
            &repo_id,
            user.user_id,
        )
        .await?;

        // A unified API key adds a library-scope ceiling: a read ceiling on this
        // library blocks the write even when the member's permission allows it.
        if !user.credential.allows_repo(&repo_id, true) {
            return Err(AppError::Forbidden);
        }

        Ok(RepoPathWrite { user, repo_id })
    }
}
