//! Extractors that combine authentication with repo permission checking.
//!
//! These extractors reduce boilerplate in handlers that need to:
//! 1. Authenticate the user (via AuthUser)
//! 2. Extract repo_id from the URL path
//! 3. Check read/write permission on the repo

use axum::extract::{FromRequestParts, Path};
use axum::http::request::Parts;
use std::sync::Arc;

use crate::AppState;
use crate::middleware::auth::AuthUser;
use axum::http::StatusCode;
use base::error::AppError;

/// Map `AuthUser`'s rejection status onto the API error it represents.
fn auth_rejection_to_app_error(status: StatusCode) -> AppError {
    match status {
        StatusCode::FORBIDDEN => AppError::Forbidden,
        _ => AppError::Unauthorized,
    }
}

/// Extractor for authenticated user + repo read permission.
///
/// Combines `AuthUser` extraction, `Path::<String>` extraction, and
/// `check_repo_read_permission` into a single step.
#[derive(Debug, Clone)]
pub struct RepoPathRead {
    pub user: AuthUser,
    pub repo_id: String,
}

/// Extractor for authenticated user + repo write permission.
///
/// Same as `RepoPathRead` but checks write permission.
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
        // Extract authenticated user. The status is preserved: a missing or
        // invalid credential is 401, while an API key that lacks the capability
        // or the library scope is 403 — collapsing both into 401 would tell a
        // client to re-authenticate when re-authenticating cannot help.
        let user = AuthUser::from_request_parts(parts, state)
            .await
            .map_err(auth_rejection_to_app_error)?;

        // Extract repo_id from path
        let Path(repo_id) = Path::<String>::from_request_parts(parts, state)
            .await
            .map_err(|_| AppError::BadRequest("missing repo_id in path".into()))?;

        // Check read permission
        crate::domain::permission::check_repo_read_permission(
            state.repos.member.as_ref(),
            &repo_id,
            user.user_id,
        )
        .await?;

        // A unified API key adds a library-scope ceiling on top of membership:
        // the key must be bound to this library (or bound to all of them).
        if let Some(authority) = &user.key
            && !authority.allows_repo(&repo_id, false)
        {
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
        // Extract authenticated user. The status is preserved: a missing or
        // invalid credential is 401, while an API key that lacks the capability
        // or the library scope is 403 — collapsing both into 401 would tell a
        // client to re-authenticate when re-authenticating cannot help.
        let user = AuthUser::from_request_parts(parts, state)
            .await
            .map_err(auth_rejection_to_app_error)?;

        // Extract repo_id from path
        let Path(repo_id) = Path::<String>::from_request_parts(parts, state)
            .await
            .map_err(|_| AppError::BadRequest("missing repo_id in path".into()))?;

        // Check write permission
        crate::domain::permission::check_repo_write_permission(
            state.repos.member.as_ref(),
            &repo_id,
            user.user_id,
        )
        .await?;

        // A unified API key adds a library-scope ceiling: a read ceiling on this
        // library blocks the write even when the member's permission allows it.
        if let Some(authority) = &user.key
            && !authority.allows_repo(&repo_id, true)
        {
            return Err(AppError::Forbidden);
        }

        Ok(RepoPathWrite { user, repo_id })
    }
}
