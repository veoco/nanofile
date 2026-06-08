use axum::{Json, Router, extract::State};
use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};
use serde::Serialize;
use std::sync::Arc;

use crate::AppState;
use crate::auth::middleware::AuthUser;
use crate::entity::starred_file;
use crate::error::AppError;

#[derive(Serialize)]
pub struct StarredFileEntry {
    pub repo_id: String,
    pub path: String,
    pub size: Option<i64>,
    pub last_modified: Option<i64>,
}

/// `GET /api2/starredfiles/`
///
/// Legacy endpoint — returns all starred files for the current user.
pub async fn list_starred_files(
    auth: AuthUser,
    State(state): State<Arc<AppState>>,
) -> Result<Json<Vec<StarredFileEntry>>, AppError> {
    let entries = starred_file::Entity::find()
        .filter(starred_file::Column::UserId.eq(auth.user_id))
        .all(state.db.as_ref())
        .await?;

    // The legacy API returns basic fields — repo_id and path are the minimum.
    let result: Vec<StarredFileEntry> = entries
        .into_iter()
        .map(|e| StarredFileEntry {
            repo_id: e.repo_id,
            path: e.path,
            size: None,
            last_modified: None,
        })
        .collect();

    Ok(Json(result))
}

pub fn starred_routes() -> Router<Arc<AppState>> {
    Router::new().route("/starredfiles/", axum::routing::get(list_starred_files))
}
