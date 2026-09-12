use axum::{
    Json,
    extract::{Query, State},
};
use serde::Deserialize;
use std::sync::Arc;

use crate::AppState;
use crate::middleware::auth::AuthUser;
use crate::service::activity::ActivityService;
use base::error::AppError;

#[derive(Deserialize)]
pub struct ActivitiesQuery {
    pub page: Option<u32>,
    pub per_page: Option<u32>,
    pub repo_id: Option<String>,
    pub op_user: Option<String>,
}

/// GET /api/v2.1/activities/
///
/// Returns paginated file activity events visible to the authenticated user.
pub async fn get_activities(
    auth: AuthUser,
    State(state): State<Arc<AppState>>,
    Query(query): Query<ActivitiesQuery>,
) -> Result<Json<serde_json::Value>, AppError> {
    // Clamp at the boundary; the service clamps again defensively because
    // `per_page` becomes the SQL LIMIT.
    let page = query
        .page
        .unwrap_or(1)
        .clamp(1, crate::service::activity::MAX_ACTIVITY_PAGE);
    let per_page = query
        .per_page
        .unwrap_or(25)
        .clamp(1, crate::service::activity::MAX_ACTIVITY_PER_PAGE);
    let result = ActivityService::get_activities(
        &state.repos,
        &state.config.server.site_url_origin(),
        auth.user_id,
        page,
        per_page,
        query.repo_id.as_deref(),
        query.op_user.as_deref(),
        &auth.repo_scope(),
    )
    .await?;

    Ok(Json(result))
}
