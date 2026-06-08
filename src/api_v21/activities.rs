use axum::{
    Json,
    extract::{Query, State},
};
use sea_orm::{ColumnTrait, EntityTrait, QueryFilter, QueryOrder};
use serde::Deserialize;
use std::sync::Arc;

use crate::AppState;
use crate::auth::middleware::AuthUser;
use crate::entity::activity;
use crate::error::AppError;

#[derive(Deserialize)]
pub struct ActivitiesQuery {
    pub page: Option<u32>,
    pub per_page: Option<u32>,
    pub repo_id: Option<String>,
}

/// GET /api/v2.1/activities/
pub async fn get_activities(
    auth: AuthUser,
    State(state): State<Arc<AppState>>,
    Query(query): Query<ActivitiesQuery>,
) -> Result<Json<serde_json::Value>, AppError> {
    let mut find = activity::Entity::find()
        .filter(activity::Column::UserId.eq(auth.user_id))
        .order_by_desc(activity::Column::CreatedAt);

    if let Some(ref repo_id) = query.repo_id {
        find = find.filter(activity::Column::RepoId.eq(repo_id));
    }

    let events = find.all(state.db.as_ref()).await?;

    let event_list: Vec<serde_json::Value> = events
        .into_iter()
        .map(|e| {
            serde_json::json!({
                "repo_id": e.repo_id,
                "commit_id": e.commit_id,
                "op_type": e.op_type,
                "obj_type": e.obj_type,
                "path": e.path,
                "time": e.created_at,
            })
        })
        .collect();

    Ok(Json(serde_json::json!({"events": event_list})))
}
