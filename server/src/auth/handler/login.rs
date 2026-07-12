use axum::{
    Json,
    body::Body,
    extract::State,
    http::{HeaderMap, HeaderName, HeaderValue, Request, StatusCode},
    response::{IntoResponse, Response},
};
use bytes::Bytes;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

use crate::AppState;
use crate::auth::middleware::AuthUser;
use crate::auth::service::login::LoginResult;
use base::error::AppError;

#[derive(Deserialize)]
pub struct LoginForm {
    pub username: String,
    pub password: String,
    pub platform: Option<String>,
    pub device_id: Option<String>,
    pub device_name: Option<String>,
    pub client_version: Option<String>,
}

#[derive(Serialize)]
pub struct LoginResponse {
    pub token: String,
}

#[derive(Serialize)]
pub struct PingResponse {
    pub email: String,
}

pub fn auth_routes() -> axum::Router<Arc<AppState>> {
    axum::Router::new()
        .route("/auth-token/", axum::routing::post(login))
        .route("/auth/ping/", axum::routing::get(ping))
        .route("/logout-device/", axum::routing::post(logout_device))
}

/// Return a seahub-compatible login error response.
fn login_error(msg: &str) -> Result<Response, AppError> {
    Ok((
        StatusCode::BAD_REQUEST,
        Json(serde_json::json!({"non_field_errors": [msg]})),
    )
        .into_response())
}

/// Parse a multipart/form-data login request body.
///
/// The seadroid client sends login credentials as multipart/form-data
/// (via Retrofit's @Multipart + @PartMap), so we need to handle this
/// in addition to JSON and URL-encoded form data.
async fn parse_multipart_login(bytes: &[u8], content_type: &str) -> Result<LoginForm, String> {
    let boundary =
        multer::parse_boundary(content_type).map_err(|e| format!("invalid boundary: {e}"))?;

    let mut form = LoginForm {
        username: String::new(),
        password: String::new(),
        platform: None,
        device_id: None,
        device_name: None,
        client_version: None,
    };

    let body_bytes = bytes.to_vec();
    let stream = futures_util::stream::once(futures_util::future::ready(
        Ok::<Bytes, multer::Error>(Bytes::from(body_bytes)),
    ));

    let mut mp = multer::Multipart::new(stream, boundary);

    while let Some(field) = mp
        .next_field()
        .await
        .map_err(|e| format!("read error: {e}"))?
    {
        let name = field.name().unwrap_or("").to_string();
        let value = field
            .text()
            .await
            .map_err(|e| format!("field text error: {e}"))?;

        match name.as_str() {
            "username" => form.username = value,
            "password" => form.password = value,
            "platform" => form.platform = Some(value),
            "device_id" => form.device_id = Some(value),
            "device_name" => form.device_name = Some(value),
            "client_version" => form.client_version = Some(value),
            // seadroid also sends "platform_version" — ignore it silently
            _ => {}
        }
    }

    if form.username.is_empty() || form.password.is_empty() {
        return Err("username and password are required".to_string());
    }

    Ok(form)
}

pub async fn login(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    req: Request<Body>,
) -> Result<Response, AppError> {
    // ── Parse body: accept both JSON and URL-encoded form data ──────────
    let (parts, body) = req.into_parts();
    let bytes = axum::body::to_bytes(body, usize::MAX)
        .await
        .map_err(|e| AppError::BadRequest(format!("Failed to read request body: {e}")))?;

    let content_type = parts
        .headers
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");

    let form: LoginForm = if content_type.starts_with("application/json") {
        serde_json::from_slice(&bytes)
            .map_err(|e| AppError::BadRequest(format!("Invalid JSON body: {e}")))?
    } else if content_type.starts_with("multipart/form-data") {
        parse_multipart_login(&bytes, content_type)
            .await
            .map_err(|e| AppError::BadRequest(format!("Invalid multipart body: {e}")))?
    } else {
        serde_urlencoded::from_bytes(&bytes)
            .map_err(|e| AppError::BadRequest(format!("Invalid form body: {e}")))?
    };

    // ── CSRF: validate Origin/Referer for browser-based requests ──────
    let origin = state.config.server.site_url_origin();
    if !crate::auth::csrf::validate_origin(&headers, &origin) {
        return login_error("Invalid request origin.");
    }

    // ── Extract client IP for rate limiting ──────────────────────────
    let client_ip = headers
        .get("x-forwarded-for")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.split(',').next().map(|s| s.trim().to_string()))
        .or_else(|| {
            headers
                .get("x-real-ip")
                .and_then(|v| v.to_str().ok())
                .map(|s| s.to_string())
        })
        .unwrap_or_else(|| "unknown".to_string());

    // Extract S2FA/OTP/trust-device headers
    let s2fa_header = headers.get("X-SEAFILE-S2FA").and_then(|v| v.to_str().ok());
    let otp_header = headers.get("X-SEAFILE-OTP").and_then(|v| v.to_str().ok());
    let trust_device = headers
        .get("X-SEAFILE-2FA-TRUST-DEVICE")
        .and_then(|v| v.to_str().ok())
        == Some("1");

    // ── Call service ─────────────────────────────────────────────────
    let svc = state.login_service();
    let login_result = svc
        .authenticate(
            &form.username,
            &form.password,
            &client_ip,
            s2fa_header,
            otp_header,
            trust_device,
            form.platform,
            form.device_id,
            form.device_name,
            form.client_version,
        )
        .await?;

    // ── Format response ──────────────────────────────────────────────
    match login_result {
        LoginResult::Success {
            api_token,
            s2fa_token,
        } => {
            let mut response = Json(LoginResponse { token: api_token }).into_response();
            if let Some(s2fa) = s2fa_token {
                response.headers_mut().insert(
                    HeaderName::from_static("x-seafile-s2fa"),
                    HeaderValue::from_str(&s2fa)
                        .unwrap_or_else(|_| HeaderValue::from_static("invalid")),
                );
            }
            Ok(response)
        }
        LoginResult::RateLimited => login_error("Too many login attempts. Please try again later."),
        LoginResult::BadCredentials => login_error("Unable to login with provided credentials."),
        LoginResult::AccountDisabled => login_error("User account is disabled."),
        LoginResult::TwoFactorRequired => {
            let mut resp_headers = HeaderMap::new();
            resp_headers.insert(
                HeaderName::from_bytes(b"X-Seafile-OTP").unwrap(),
                HeaderValue::from_static("required"),
            );
            let body = serde_json::json!({
                "non_field_errors": ["Two factor auth token is missing."],
            });
            Ok((StatusCode::BAD_REQUEST, resp_headers, Json(body)).into_response())
        }
        LoginResult::TwoFactorInvalid => {
            let mut resp_headers = HeaderMap::new();
            resp_headers.insert(
                HeaderName::from_bytes(b"X-Seafile-OTP").unwrap(),
                HeaderValue::from_static("required"),
            );
            let body = serde_json::json!({
                "non_field_errors": ["Two factor auth token is invalid."],
            });
            Ok((StatusCode::BAD_REQUEST, resp_headers, Json(body)).into_response())
        }
    }
}

/// Public ping — returns "pong" (no auth required).
/// Matches the original seahub's GET /api2/ping/ behavior.
pub async fn public_ping() -> impl IntoResponse {
    (StatusCode::OK, Json("pong"))
}

/// Authenticated ping — returns the authenticated user's email.
/// Serves GET /api2/auth/ping/.
pub async fn ping(auth: AuthUser) -> Result<Json<PingResponse>, AppError> {
    Ok(Json(PingResponse { email: auth.email }))
}

/// `POST /api2/logout-device/`
///
/// Invalidates the current API token, logging out the device.
pub async fn logout_device(
    _auth: AuthUser,
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Result<Json<serde_json::Value>, AppError> {
    // Extract the token from Authorization header
    let token_str = headers
        .get("Authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| {
            v.strip_prefix("Token ")
                .or_else(|| v.strip_prefix("Bearer "))
        })
        .ok_or(AppError::Unauthorized)?;

    // Delete the api_token record
    state.repos.api_token.delete_by_token(token_str).await?;

    Ok(Json(serde_json::json!({"success": true})))
}
