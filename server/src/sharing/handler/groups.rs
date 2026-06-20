use axum::{
    Json,
    extract::{Query, State},
};
use serde::Deserialize;
use std::sync::Arc;

use crate::AppState;
use crate::auth::middleware::AuthUser;
use crate::error::AppError;
use crate::sharing::service::group;

/// GET /api2/groups/
pub async fn list_groups(
    auth: AuthUser,
    State(state): State<Arc<AppState>>,
) -> Result<Json<Vec<serde_json::Value>>, AppError> {
    let result = group::list_groups(state.db.as_ref(), &state.repos, auth.user_id).await?;
    Ok(Json(result))
}

/// GET /api2/groupandcontacts/
pub async fn groups_and_contacts(
    auth: AuthUser,
    State(state): State<Arc<AppState>>,
) -> Result<Json<serde_json::Value>, AppError> {
    let result = group::groups_and_contacts(state.db.as_ref(), &state.repos, auth.user_id).await?;
    Ok(Json(result))
}

/// GET /api2/search-user/?q=
#[derive(Deserialize)]
pub struct SearchUserQuery {
    pub q: Option<String>,
}

pub async fn search_user(
    _auth: AuthUser,
    State(state): State<Arc<AppState>>,
    Query(query): Query<SearchUserQuery>,
) -> Result<Json<Vec<serde_json::Value>>, AppError> {
    let q = query.q.unwrap_or_default();
    let result = group::search_user(state.db.as_ref(), &state.repos, &q).await?;
    Ok(Json(result))
}
