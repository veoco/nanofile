use axum::{
    Json, Router,
    extract::{Path, State},
};
use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};
use serde::Serialize;
use std::sync::Arc;

use crate::AppState;
use crate::auth::middleware::AuthUser;
use crate::entity::user;
use crate::error::AppError;

#[derive(Serialize)]
pub struct AvatarResponse {
    pub url: String,
    pub is_default: bool,
    pub mtime: i64,
}

/// `GET /api2/avatars/user/{email}/resized/{size}/`
///
/// Returns avatar URL for a user. Nanofile's default behavior is to
/// return a standard default avatar (rendered by the client).
pub async fn get_avatar(
    _auth: AuthUser,
    State(state): State<Arc<AppState>>,
    Path((email, _size)): Path<(String, String)>,
) -> Result<Json<AvatarResponse>, AppError> {
    // Verify user exists
    let _user = user::Entity::find()
        .filter(user::Column::Email.eq(&email))
        .one(state.db.as_ref())
        .await?
        .ok_or_else(|| AppError::NotFound("user not found".into()))?;

    Ok(Json(AvatarResponse {
        url: format!("/avatars/user/{}/resized/default", email),
        is_default: true,
        mtime: 0,
    }))
}

pub fn avatar_routes() -> Router<Arc<AppState>> {
    Router::new().route(
        "/avatars/user/{email}/resized/{size}/",
        axum::routing::get(get_avatar),
    )
}
