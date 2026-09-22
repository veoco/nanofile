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
    // The token minted for each library belongs to the requesting device when
    // the request identifies one, so the credential inventory can show it under
    // that device at once.
    let peer = _auth.device.as_ref().map(|device| device.peer());
    let repos = state
        .sync_service()
        .accessible_repos(
            _auth.user_id,
            state.config().auth.sync_token_ttl_days,
            &_auth.repo_scope(),
            peer.as_ref(),
        )
        .await?;
    Ok(Json(repos))
}

pub fn accessible_repos_routes() -> Router<Arc<AppState>> {
    Router::new().route("/accessible-repos", axum::routing::get(accessible_repos))
}
