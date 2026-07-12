use axum::{
    Json, Router,
    extract::{Path, State},
};
use serde::Serialize;
use std::sync::Arc;

use crate::AppState;
use crate::auth::middleware::SyncAuth;
use base::error::AppError;

/// Response for the JWT token endpoint.
/// Seafile format: `{"jwt_token": "..."}`
#[derive(Serialize)]
pub struct JwtTokenResponse {
    pub jwt_token: String,
}

/// GET /seafhttp/repo/{repo_id}/jwt-token
///
/// Returns a JWT token for accessing the notification server.
/// The token encodes the repo_id, username (email), and expiration time.
/// Signed with the configured notification private key using HS256.
///
/// This is the endpoint that seaf-cli calls to get a JWT token before
/// connecting to the notification WebSocket server.
pub async fn get_jwt_token(
    State(state): State<Arc<AppState>>,
    auth: SyncAuth,
    Path(repo_id): Path<String>,
) -> Result<Json<JwtTokenResponse>, AppError> {
    // Check that notification server is configured.
    if state.notification_manager.is_none() {
        return Err(AppError::NotFound("notification not configured".into()));
    }

    let private_key = &state.config.notification.private_key;
    if private_key.is_empty() {
        return Err(AppError::Internal(
            "notification private key not configured".into(),
        ));
    }

    // Get the user's email from the database.
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

    let header = jsonwebtoken::Header::new(jsonwebtoken::Algorithm::HS256);
    let key = jsonwebtoken::EncodingKey::from_secret(private_key.as_bytes());

    let token = jsonwebtoken::encode(&header, &claims, &key)
        .map_err(|e| AppError::Internal(format!("JWT encoding error: {}", e)))?;

    Ok(Json(JwtTokenResponse { jwt_token: token }))
}

pub fn jwt_token_routes() -> Router<Arc<AppState>> {
    Router::new().route("/{repo_id}/jwt-token", axum::routing::get(get_jwt_token))
}
