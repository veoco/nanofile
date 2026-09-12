use axum::Json;
use axum::extract::State;
use axum::http::HeaderMap;
use std::sync::Arc;

use crate::AppState;
use crate::handler::ok_json;
use base::error::AppError;

/// POST /api2/device-wiped/
///
/// A wiped device reports itself so the server drops the credentials it holds.
/// The two official callers authenticate differently and **both** shapes have
/// to work:
///
/// * desktop Qt: `application/x-www-form-urlencoded` with a `token` form field
///   and *no* `Authorization` header
///   (`seafile-client/src/api/requests.cpp:1225-1235`);
/// * Android: an empty multipart POST with `Authorization: Token <token>` and
///   no body fields (`seadroid/.../AccountService.java:24-26`, token added by
///   `TokenInterceptor`).
///
/// Requiring only the form field meant Android's wipe report was rejected and
/// its credentials were never revoked; requiring only the header would break
/// the desktop client. Scoping by the reporting token's owner prevents
/// cross-user session revocation.
pub async fn device_wiped(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    body: String,
) -> Result<Json<serde_json::Value>, AppError> {
    let token = match extract_authorization_token(&headers) {
        Some(t) => t,
        None => infra::common::util::extract_body_field(body.as_bytes(), "token")
            .ok_or_else(|| AppError::BadRequest("token required".into()))?,
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
    // The device's `/download-api/…`, `/upload-api/…` capability URLs are
    // credentials too, and they are not addressed by device id, so drop all of
    // the user's outstanding ones.
    state.token_manager.revoke_user(token_record.user_id);

    Ok(ok_json())
}

/// `Authorization: Token <t>` / `Bearer <t>`, as the mobile clients send it.
fn extract_authorization_token(headers: &HeaderMap) -> Option<String> {
    let raw = headers.get("authorization")?.to_str().ok()?;
    let token = raw
        .strip_prefix("Token ")
        .or_else(|| raw.strip_prefix("Bearer "))?;
    if token.is_empty() {
        return None;
    }
    Some(token.to_string())
}
