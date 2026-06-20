use axum::{
    Router,
    extract::{Path, Query, State},
    http::StatusCode,
};
use serde::Deserialize;
use std::sync::Arc;

use crate::AppState;
use crate::auth::middleware::SyncAuth;
use crate::error::AppError;

/// GET /seafhttp/repo/{repo_id}/quota-check/?delta={delta}
///
/// Called by seaf-daemon at the start of every upload to check whether the
/// upload would exceed the server's quota. Real seafile-server returns:
/// - 200 OK (quota allows the delta)
/// - 400 Bad Request (invalid delta parameter)
/// - 443 No Quota (quota exceeded)
///
/// Nanofile doesn't enforce quotas, so we always return 200.
pub fn quota_routes() -> Router<Arc<AppState>> {
    Router::new().route("/{repo_id}/quota-check/", axum::routing::get(check_quota))
}

#[derive(Deserialize)]
pub struct QuotaCheckQuery {
    pub delta: Option<String>,
}

async fn check_quota(
    State(_state): State<Arc<AppState>>,
    _auth: SyncAuth,
    Path(_repo_id): Path<String>,
    Query(_query): Query<QuotaCheckQuery>,
) -> Result<StatusCode, AppError> {
    // Nanofile doesn't enforce quotas — always allow.
    Ok(StatusCode::OK)
}
