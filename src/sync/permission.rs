use axum::{
    Router,
    extract::{Path, Query, State},
    http::StatusCode,
};
use sea_orm::EntityTrait;
use serde::Deserialize;
use std::sync::Arc;

use crate::AppState;
use crate::auth::middleware::SyncAuth;
use crate::entity::repo;
use crate::error::AppError;

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
    repo::Entity::find_by_id(&repo_id)
        .one(state.db.as_ref())
        .await?
        .ok_or(AppError::RepoDeleted)?;

    // Check permission based on operation type.
    // seaf-daemon sends op=upload or op=download.
    match query.op.as_deref() {
        Some("upload") => {
            crate::storage::check_repo_write_permission(state.db.as_ref(), &repo_id, _auth.user_id)
                .await?;
        }
        _ => {
            // Default to read permission check (covers download + unknown ops).
            crate::storage::check_repo_read_permission(state.db.as_ref(), &repo_id, _auth.user_id)
                .await?;
        }
    }

    Ok(StatusCode::OK)
}
