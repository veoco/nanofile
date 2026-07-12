use axum::{
    Router,
    extract::{Path, Query, State},
    http::StatusCode,
};
use serde::Deserialize;
use std::sync::Arc;

use crate::AppState;
use crate::middleware::auth::SyncAuth;
use base::error::AppError;

#[derive(Deserialize)]
pub struct PermissionQuery {
    op: Option<String>,
}

pub fn permission_routes() -> Router<Arc<AppState>> {
    Router::new().route(
        "/{repo_id}/permission-check/",
        axum::routing::get(permission_check),
    )
}

/// seafile-server returns HTTP 200 with empty body on success,
/// HTTP 403 for no permission, HTTP 444 for deleted repo,
/// HTTP 445 for corrupted repo.
///
/// Checks the user's permission level on the repo. For "upload" ops,
/// requires write (rw) permission. For "download" ops, requires read (r).
pub async fn permission_check(
    State(state): State<Arc<AppState>>,
    _auth: SyncAuth,
    Path(repo_id): Path<String>,
    Query(query): Query<PermissionQuery>,
) -> Result<StatusCode, AppError> {
    // Verify repo exists — if not, return 444 (repo deleted) as seaf-daemon expects
    if !state.sync_service().repo_exists(&repo_id).await? {
        return Err(AppError::RepoDeleted);
    }

    // Check permission based on operation type.
    // seaf-daemon sends op=upload or op=download.
    match query.op.as_deref() {
        Some("upload") => {
            crate::domain::permission::check_repo_write_permission(
                state.repos.member.as_ref(),
                &repo_id,
                _auth.user_id,
            )
            .await?;
        }
        _ => {
            // Default to read permission check (covers download + unknown ops).
            crate::domain::permission::check_repo_read_permission(
                state.repos.member.as_ref(),
                &repo_id,
                _auth.user_id,
            )
            .await?;
        }
    }

    Ok(StatusCode::OK)
}
