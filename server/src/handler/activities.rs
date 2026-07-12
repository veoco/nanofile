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
    pub avatar_size: Option<u32>,
}

/// GET /api/v2.1/activities/
///
/// Returns paginated file activity events visible to the authenticated user.
pub async fn get_activities(
    auth: AuthUser,
    State(state): State<Arc<AppState>>,
    Query(query): Query<ActivitiesQuery>,
) -> Result<Json<serde_json::Value>, AppError> {
    let result = ActivityService::get_activities(
        &state.repos,
        auth.user_id,
        query.page.unwrap_or(1),
        query.per_page.unwrap_or(25),
        query.repo_id.as_deref(),
        query.op_user.as_deref(),
    )
    .await?;

    Ok(Json(result))
}
