use axum::{Json, Router};
use serde::Serialize;
use std::sync::Arc;

use crate::AppState;
use crate::auth::middleware::AuthUser;
use crate::error::AppError;

#[derive(Serialize)]
pub struct UnseenMessagesResponse {
    pub count: i32,
}

/// `GET /api2/unseen_messages/`
///
/// Returns count of unseen Seahub notifications.
/// Nanofile doesn't have a notification system, so always returns 0.
pub async fn unseen_messages(_auth: AuthUser) -> Result<Json<UnseenMessagesResponse>, AppError> {
    Ok(Json(UnseenMessagesResponse { count: 0 }))
}

pub fn notifications_routes() -> Router<Arc<AppState>> {
    Router::new().route("/unseen_messages/", axum::routing::get(unseen_messages))
}
