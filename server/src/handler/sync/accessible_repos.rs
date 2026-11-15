use axum::{Json, Router, extract::State};
use std::sync::Arc;

use crate::AppState;
use crate::middleware::auth::SyncAuth;
use base::error::AppError;

/// `GET /seafhttp/accessible-repos`
///
/// The response shape is the fileserver's, not the REST repo shape: SeaDrive
/// parses it with `parse_repo_list()` (see [`crate::service::sync::AccessibleRepo`]).
pub async fn accessible_repos(
    auth: SyncAuth,
    State(state): State<Arc<AppState>>,
) -> Result<Json<Vec<crate::service::sync::AccessibleRepo>>, AppError> {
    let repos = state
        .sync_service()
        .accessible_repos(auth.user_id, &auth.repo_scope())
        .await?;
    Ok(Json(repos))
}

pub fn accessible_repos_routes() -> Router<Arc<AppState>> {
    // The daemon calls `/seafhttp/accessible-repos/?repo_id=…` with a trailing
    // slash (seadrive-fuse sync-mgr.c), while the fileserver also serves the
    // slash-less form. Register both; Axum matches paths exactly.
    Router::new()
        .route("/accessible-repos", axum::routing::get(accessible_repos))
        .route("/accessible-repos/", axum::routing::get(accessible_repos))
}
