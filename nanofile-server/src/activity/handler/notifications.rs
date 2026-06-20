use axum::{Json, Router};
use serde::Serialize;
use std::sync::Arc;

use crate::AppState;
use crate::activity::service::notification_service;
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
    let count = notification_service::get_unseen_messages();
    Ok(Json(UnseenMessagesResponse { count }))
}

pub fn notifications_routes() -> Router<Arc<AppState>> {
    Router::new().route("/unseen_messages/", axum::routing::get(unseen_messages))
}
