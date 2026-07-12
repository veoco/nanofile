use axum::{Json, Router, extract::State};
use std::sync::Arc;

use crate::AppState;
use crate::middleware::auth::SyncAuth;
use base::error::AppError;

/// `GET /seafhttp/accessible-repos`
pub async fn accessible_repos(
    _auth: SyncAuth,
    State(state): State<Arc<AppState>>,
) -> Result<Json<Vec<crate::service::sync::AccessibleRepo>>, AppError> {
    let repos = state.sync_service().accessible_repos(_auth.user_id).await?;
    Ok(Json(repos))
}

pub fn accessible_repos_routes() -> Router<Arc<AppState>> {
    Router::new().route("/accessible-repos", axum::routing::get(accessible_repos))
}
