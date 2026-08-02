use axum::{
    Json,
    extract::{Query, State},
};
use serde::Deserialize;
use std::sync::Arc;

use crate::AppState;
use crate::middleware::auth::AuthUser;
use crate::service::sharing::group;
use base::error::AppError;

/// GET /api2/groups/
pub async fn list_groups(
    auth: AuthUser,
    State(state): State<Arc<AppState>>,
) -> Result<Json<Vec<serde_json::Value>>, AppError> {
    let result = group::list_groups(&state.repos, auth.user_id).await?;
    Ok(Json(result))
}

/// GET /api/v2.1/groups/
///
/// Required by the seadroid library-list load chain (`getGroupsAsync`); must
/// return 200 + an array (empty is fine) rather than 404. `group_quota_usage`
/// is returned as integer 0 to keep the Android `long` parser happy.
#[derive(Deserialize)]
pub struct GroupsV21Query {
    pub with_repos: Option<i64>,
    #[allow(dead_code)]
    pub avatar_size: Option<i64>,
}

pub async fn list_groups_v21(
    auth: AuthUser,
    State(state): State<Arc<AppState>>,
    Query(query): Query<GroupsV21Query>,
) -> Result<Json<Vec<serde_json::Value>>, AppError> {
    let with_repos = query.with_repos.unwrap_or(0);
    if with_repos != 0 && with_repos != 1 {
        return Err(AppError::BadRequest("with_repos invalid".into()));
    }
    let result = group::list_groups_v21(&state.repos, auth.user_id, with_repos == 1).await?;
    Ok(Json(result))
}

/// GET /api2/groupandcontacts/
pub async fn groups_and_contacts(
    auth: AuthUser,
    State(state): State<Arc<AppState>>,
) -> Result<Json<serde_json::Value>, AppError> {
    let result = group::groups_and_contacts(&state.repos, auth.user_id).await?;
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
    let result = group::search_user(&state.repos, &q).await?;
    Ok(Json(result))
}
