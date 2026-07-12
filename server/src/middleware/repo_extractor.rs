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
use base::error::AppError;

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
        // Extract authenticated user
        let user = AuthUser::from_request_parts(parts, state)
            .await
            .map_err(|_| AppError::Unauthorized)?;

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

        Ok(RepoPathRead { user, repo_id })
    }
}

impl FromRequestParts<Arc<AppState>> for RepoPathWrite {
    type Rejection = AppError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &Arc<AppState>,
    ) -> Result<Self, Self::Rejection> {
        // Extract authenticated user
        let user = AuthUser::from_request_parts(parts, state)
            .await
            .map_err(|_| AppError::Unauthorized)?;

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

        Ok(RepoPathWrite { user, repo_id })
    }
}
