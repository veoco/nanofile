use axum::{
    Json, Router,
    extract::{Path, Query, State},
};
use serde::Deserialize;
use std::sync::Arc;

use crate::AppState;
use crate::auth::middleware::AuthUser;
use crate::error::AppError;
use crate::repo::service::history_service::HistoryService;

#[derive(Deserialize)]
pub struct HistoryQuery {
    pub commit_id: String,
}

/// `GET /api2/repo_history_changes/{repo_id}/?commit_id=`
///
/// Returns the file changes introduced by a specific commit.
pub async fn repo_history_changes(
    _auth: AuthUser,
    State(state): State<Arc<AppState>>,
    Path(repo_id): Path<String>,
    Query(query): Query<HistoryQuery>,
) -> Result<Json<crate::repo::service::history_service::HistoryChangesResponse>, AppError> {
    let response = HistoryService::get_history_changes(
        state.db.as_ref(),
        &state.repos,
        &repo_id,
        &query.commit_id,
    )
    .await?;

    Ok(Json(response))
}

pub fn repo_history_routes() -> Router<Arc<AppState>> {
    Router::new().route(
        "/repo_history_changes/{repo_id}/",
        axum::routing::get(repo_history_changes),
    )
}
