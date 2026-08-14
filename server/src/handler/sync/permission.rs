use axum::{
    Json, Router,
    extract::{Path, Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
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
///
/// On a 403 the daemon parses the body's `reason` field to distinguish a
/// read-only share ("no write permission") from full access-denied — without
/// it a read-only shared repo would be reported as a generic access error.
pub async fn permission_check(
    State(state): State<Arc<AppState>>,
    _auth: SyncAuth,
    Path(repo_id): Path<String>,
    Query(query): Query<PermissionQuery>,
) -> Result<Response, AppError> {
    // Verify repo exists — if not, return 444 (repo deleted) as seaf-daemon expects
    if !state.sync_service().repo_exists(&repo_id).await? {
        return Err(AppError::RepoDeleted);
    }

    // Check permission based on operation type.
    // seaf-daemon sends op=upload or op=download.
    match query.op.as_deref() {
        Some("upload") => {
            match crate::domain::permission::check_repo_write_permission(
                state.repos.member.as_ref(),
                &repo_id,
                _auth.user_id,
            )
            .await
            {
                Ok(()) => {}
                // Read-only members (and any non-write access) are reported to
                // the daemon as "no write permission" rather than a generic 403.
                Err(AppError::Forbidden) => {
                    return Ok((
                        StatusCode::FORBIDDEN,
                        Json(serde_json::json!({ "reason": "no write permission" })),
                    )
                        .into_response());
                }
                Err(e) => return Err(e),
            }
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

    Ok(StatusCode::OK.into_response())
}
