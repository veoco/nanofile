use axum::Json;
use axum::extract::State;
use rand::Rng;
use sea_orm::{EntityTrait, Set};
use std::sync::Arc;

use crate::AppState;
use crate::auth::middleware::AuthUser;
use crate::entity::client_login_token;
use crate::error::AppError;

/// POST /api2/client-login/
///
/// Generate a one-time token for the "view on website" feature.
/// The sync client calls this to get a token, then opens a browser
/// to /client-login/?token=... which auto-authenticates the user.
/// Token is valid for 30 seconds (matching Seahub behavior).
pub async fn client_login(
    auth: AuthUser,
    State(state): State<Arc<AppState>>,
) -> Result<Json<serde_json::Value>, AppError> {
    // Generate 32-char hex token (matching Seahub's ClientLoginToken)
    let mut raw = [0u8; 16];
    rand::rng().fill_bytes(&mut raw);
    let token = hex::encode(raw);
    let now = chrono::Utc::now().timestamp();

    client_login_token::Entity::insert(client_login_token::ActiveModel {
        token: Set(token.clone()),
        username: Set(auth.email.clone()),
        created_at: Set(now),
    })
    .exec(state.db.as_ref())
    .await?;

    Ok(Json(serde_json::json!({"token": token})))
}
