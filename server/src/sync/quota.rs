use axum::{
    Router,
    extract::{Path, Query, State},
    http::StatusCode,
};
use serde::Deserialize;
use std::sync::Arc;

use crate::AppState;
use crate::auth::middleware::SyncAuth;
use base::error::AppError;

/// GET /seafhttp/repo/{repo_id}/quota-check/?delta={delta}
///
/// Called by seaf-daemon at the start of every upload to check whether the
/// upload would exceed the server's quota. Real seafile-server returns:
/// - 200 OK (quota allows the delta)
/// - 400 Bad Request (invalid delta parameter)
/// - 443 No Quota (quota exceeded)
pub fn quota_routes() -> Router<Arc<AppState>> {
    Router::new().route("/{repo_id}/quota-check/", axum::routing::get(check_quota))
}

#[derive(Deserialize)]
pub struct QuotaCheckQuery {
    pub delta: Option<String>,
}

async fn check_quota(
    State(state): State<Arc<AppState>>,
    _auth: SyncAuth,
    Path(repo_id): Path<String>,
    Query(query): Query<QuotaCheckQuery>,
) -> Result<StatusCode, AppError> {
    let delta = match query.delta {
        Some(ref d) => d
            .parse::<i64>()
            .map_err(|_| AppError::BadRequest("invalid delta parameter".into()))?,
        None => 0,
    };

    if delta <= 0 {
        return Ok(StatusCode::OK);
    }

    // Look up the repo owner to check their quota.
    let repo_record = state
        .repos
        .repo
        .find_by_id(&repo_id)
        .await?
        .ok_or_else(|| AppError::NotFound("repo not found".into()))?;

    crate::web::quota::check_upload_quota(
        &state.repos,
        repo_record.owner_id,
        delta,
        state.config.storage.max_storage_bytes,
    )
    .await?;

    Ok(StatusCode::OK)
}
