use axum::{
    Router,
    extract::{Path, State},
    http::StatusCode,
};
use sea_orm::EntityTrait;
use std::sync::Arc;

use crate::AppState;
use crate::auth::middleware::SyncAuth;
use crate::entity::repo;
use crate::error::AppError;

pub fn permission_routes() -> Router<Arc<AppState>> {
    Router::new().route(
        "/{repo_id}/permission-check/",
        axum::routing::get(permission_check),
    )
}

/// seafile-server returns HTTP 200 with empty body on success,
/// HTTP 444 for deleted repo, HTTP 445 for corrupted repo.
/// The daemon checks for these specific status codes.
pub async fn permission_check(
    State(state): State<Arc<AppState>>,
    _auth: SyncAuth,
    Path(repo_id): Path<String>,
) -> Result<StatusCode, AppError> {
    // Verify repo exists — if not, return 444 (repo deleted) as seaf-daemon expects
    repo::Entity::find_by_id(&repo_id)
        .one(state.db.as_ref())
        .await?
        .ok_or(AppError::RepoDeleted)?;

    Ok(StatusCode::OK)
}
