use axum::{
    Json, Router,
    extract::{Path, State},
};
use serde::Serialize;
use std::sync::Arc;

use crate::AppState;
use crate::middleware::auth::SyncAuth;
use base::error::AppError;

/// Seafile format: `{"jwt_token": "..."}`
#[derive(Serialize)]
pub struct JwtTokenResponse {
    pub jwt_token: String,
}

/// GET /seafhttp/repo/{repo_id}/jwt-token
///
/// A JWT for the notification server, encoding the repo_id, username (email) and
/// expiry and signed HS256 with the configured notification private key. seaf-cli
/// calls this before connecting to the notification WebSocket.
pub async fn get_jwt_token(
    State(state): State<Arc<AppState>>,
    auth: SyncAuth,
    Path(repo_id): Path<String>,
) -> Result<Json<JwtTokenResponse>, AppError> {
    if state.notification_manager.is_none() {
        return Err(AppError::NotFound("notification not configured".into()));
    }

    let private_key = &state.config().notification.private_key;
    if private_key.is_empty() {
        return Err(AppError::Internal(
            "notification private key not configured".into(),
        ));
    }

    // The JWT grants access to this repo's notification stream, so it must only be
    // issued to a member (defense in depth on the `SyncAuth` path/repo binding).
    crate::domain::permission::check_repo_read_permission(
        state.repos.member.as_ref(),
        &repo_id,
        auth.user_id,
    )
    .await?;

    let user = state
        .repos
        .user
        .find_by_id(auth.user_id)
        .await?
        .ok_or_else(|| AppError::NotFound("user not found".into()))?;

    let now = chrono::Utc::now().timestamp();
    // Seafile uses 3 days (259200 seconds) for the notification JWT expiration.
    let exp = now + 259200;

    let claims = serde_json::json!({
        "repo_id": repo_id,
        "username": user.email,
        "exp": exp,
    });

    // Sign with the subscription-specific subkey, never the raw notification key: the
    // raw key also authorises posting events, and this token goes to every member.
    let sub_key = crate::notification::keys::subscription_key(private_key);

    let token = crate::service::auth::hs256::encode(&claims, &sub_key)
        .map_err(|e| AppError::Internal(format!("JWT encoding error: {e}")))?;

    Ok(Json(JwtTokenResponse { jwt_token: token }))
}

pub fn jwt_token_routes() -> Router<Arc<AppState>> {
    Router::new().route("/{repo_id}/jwt-token", axum::routing::get(get_jwt_token))
}
