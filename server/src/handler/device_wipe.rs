use axum::Json;
use axum::extract::State;
use axum::http::HeaderMap;
use std::sync::Arc;

use crate::AppState;
use crate::handler::ok_json;
use base::error::AppError;

/// POST /api2/device-wiped/
///
/// A wiped device reports itself so the server drops the credentials it holds. Both
/// official shapes must work: desktop Qt sends `application/x-www-form-urlencoded` with
/// a `token` field and no `Authorization` header
/// (`seafile-client/src/api/requests.cpp:1225-1235`), Android an empty multipart POST
/// with `Authorization: Token <token>` (`AccountService.java:24-26`). Scoping by the
/// reporting token's owner prevents cross-user session revocation.
pub async fn device_wiped(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    body: String,
) -> Result<Json<serde_json::Value>, AppError> {
    let token = match crate::middleware::auth::bearer_token(&headers) {
        crate::middleware::auth::BearerToken::Token(token) => token.to_string(),
        // A header that names our scheme but has no token is a failed credential,
        // not "no header": do not silently switch to the body field.
        crate::middleware::auth::BearerToken::Malformed => return Err(AppError::Unauthorized),
        crate::middleware::auth::BearerToken::Missing => {
            infra::common::util::extract_body_field(body.as_bytes(), "token")
                .ok_or_else(|| AppError::BadRequest("token required".into()))?
        }
    };

    let token_record = state
        .repos
        .api_token
        .find_by_token(&token)
        .await?
        .ok_or_else(|| AppError::BadRequest("invalid token".into()))?;

    // An expired token must not be able to trigger session revocation even
    // though it can no longer authenticate (matches middleware/auth.rs).
    if matches!(
        token_record.expires_at,
        Some(exp) if chrono::Utc::now().timestamp() > exp
    ) {
        return Err(AppError::BadRequest("token expired".into()));
    }
    // A 2FA-pending token is not a full credential and must not revoke sessions.
    if token_record.is_pending {
        return Err(AppError::BadRequest("token is pending".into()));
    }

    let device_id = token_record
        .device_id
        .clone()
        .ok_or_else(|| AppError::BadRequest("token has no device".into()))?;

    let svc = state.sso_service();
    svc.device_wiped(token_record.user_id, &device_id).await?;
    // The device's `/files/…` and `/upload-api/…` capability
    // URLs are credentials too, and they are not addressed by device id, so drop
    // all of the user's outstanding ones.
    state.token_manager.revoke_user(token_record.user_id);

    Ok(ok_json())
}
