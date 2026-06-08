use axum::{
    Form, Json,
    extract::State,
    http::{HeaderMap, HeaderName, HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
};
use sea_orm::{ActiveModelTrait, ColumnTrait, EntityTrait, QueryFilter};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

use crate::AppState;
use crate::auth::middleware::AuthUser;
use crate::auth::password::verify_password;
use crate::auth::token::generate_api_token;
use crate::entity::{api_token, user};
use crate::error::AppError;

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

pub async fn login(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Form(form): Form<LoginForm>,
) -> Result<Response, AppError> {
    let user = user::Entity::find()
        .filter(user::Column::Email.eq(&form.username))
        .one(state.db.as_ref())
        .await?
        .ok_or(AppError::Unauthorized)?;

    if !verify_password(
        &form.password,
        &user.password_hash,
        state.config.auth.password_hash_iterations,
    ) {
        return Err(AppError::Unauthorized);
    }

    if !user.is_active {
        return Err(AppError::Forbidden);
    }

    let two_fa = crate::entity::user_2fa::Entity::find_by_id(user.id)
        .one(state.db.as_ref())
        .await?;

    // Track whether we can skip 2FA (valid S2FA device trust token provided).
    let mut skip_2fa = false;
    // Track whether we issued a new S2FA token to return in the response header.
    let mut issued_s2fa_token: Option<String> = None;

    if let Some(tfa) = two_fa
        && tfa.enabled
    {
        // --- S2FA device trust token check ---
        // If the client sends a stored S2FA token (from a previous "remember
        // this device" login), validate it and skip the OTP challenge entirely.
        let s2fa_header = headers.get("X-SEAFILE-S2FA").and_then(|v| v.to_str().ok());
        if let Some(s2fa_token) = s2fa_header {
            // Clean up expired tokens for this user.
            let now = chrono::Utc::now().timestamp();
            crate::entity::s2fa_token::Entity::delete_many()
                .filter(crate::entity::s2fa_token::Column::UserId.eq(user.id))
                .filter(crate::entity::s2fa_token::Column::ExpiresAt.lt(now))
                .exec(state.db.as_ref())
                .await?;

            // Look up the provided token.
            let stored = crate::entity::s2fa_token::Entity::find()
                .filter(crate::entity::s2fa_token::Column::Token.eq(s2fa_token))
                .filter(crate::entity::s2fa_token::Column::UserId.eq(user.id))
                .one(state.db.as_ref())
                .await?;

            if let Some(stored) = stored
                && stored.expires_at > now
            {
                // Valid S2FA token — skip the entire 2FA challenge.
                skip_2fa = true;
            }
            // Invalid or expired token: fall through to normal OTP check.
        }

        if !skip_2fa {
            // --- Normal OTP-based 2FA check ---
            let otp = headers.get("X-SEAFILE-OTP").and_then(|v| v.to_str().ok());

            match otp {
                Some(otp_code) => {
                    let totp = crate::auth::totp::TotpManager::create_totp(
                        &tfa.totp_secret,
                        &user.email,
                        "",
                    )
                    .map_err(|e| AppError::Internal(e.to_string()))?;
                    if !crate::auth::totp::TotpManager::verify_code(&totp, otp_code) {
                        return Err(AppError::TwoFactorInvalid);
                    }

                    // If the client requested device trust ("remember this device"),
                    // generate an S2FA token and return it in the response header.
                    let trust_device = headers
                        .get("X-SEAFILE-2FA-TRUST-DEVICE")
                        .and_then(|v| v.to_str().ok())
                        == Some("1");
                    if trust_device {
                        let s2fa_token_value = crate::auth::s2fa::generate_s2fa_token();
                        let now = chrono::Utc::now().timestamp();
                        let s2fa_model = crate::entity::s2fa_token::ActiveModel {
                            id: sea_orm::NotSet,
                            user_id: sea_orm::Set(user.id),
                            token: sea_orm::Set(s2fa_token_value.clone()),
                            device_id: sea_orm::Set(form.device_id.clone()),
                            device_name: sea_orm::Set(form.device_name.clone()),
                            created_at: sea_orm::Set(now),
                            expires_at: sea_orm::Set(now + crate::auth::s2fa::S2FA_TTL_SECONDS),
                        };
                        crate::entity::s2fa_token::Entity::insert(s2fa_model)
                            .exec(state.db.as_ref())
                            .await?;
                        issued_s2fa_token = Some(s2fa_token_value);
                    }
                }
                None => {
                    // Return 401 with X-Seafile-OTP: required header so the desktop
                    // client knows to prompt the user for a TOTP code.
                    let mut resp_headers = HeaderMap::new();
                    resp_headers.insert(
                        header::CONTENT_TYPE,
                        HeaderValue::from_static("application/json"),
                    );
                    resp_headers.insert(
                        HeaderName::from_bytes(b"X-Seafile-OTP").unwrap(),
                        HeaderValue::from_static("required"),
                    );
                    let body = serde_json::json!({
                        "error_msg": "Two factor auth token is missing.",
                        "error_code": 401,
                    });
                    let mut resp =
                        (StatusCode::UNAUTHORIZED, resp_headers, Json(body)).into_response();
                    // Re-insert Content-Type after into_response in case it was overwritten.
                    resp.headers_mut().insert(
                        header::CONTENT_TYPE,
                        HeaderValue::from_static("application/json"),
                    );
                    return Ok(resp);
                }
            }
        }
    }

    let token_value = generate_api_token();
    let now = chrono::Utc::now().timestamp();

    let token_model = api_token::ActiveModel {
        id: sea_orm::NotSet,
        user_id: sea_orm::Set(user.id),
        token: sea_orm::Set(token_value.clone()),
        created_at: sea_orm::Set(now),
        expires_at: sea_orm::Set(None),
        device_id: sea_orm::Set(form.device_id),
        platform: sea_orm::Set(form.platform),
        device_name: sea_orm::Set(form.device_name),
        client_version: sea_orm::Set(form.client_version),
    };
    api_token::Entity::insert(token_model)
        .exec(state.db.as_ref())
        .await?;

    let mut user_active: user::ActiveModel = user.into();
    user_active.last_login_at = sea_orm::Set(Some(now));
    user_active.update(state.db.as_ref()).await?;

    // Include S2FA token in response header if a new one was issued.
    let mut response = Json(LoginResponse { token: token_value }).into_response();
    if let Some(s2fa_token) = issued_s2fa_token {
        response.headers_mut().insert(
            HeaderName::from_bytes(b"X-Seafile-S2FA").unwrap(),
            HeaderValue::from_str(&s2fa_token).unwrap(),
        );
    }
    Ok(response)
}

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
    api_token::Entity::delete_many()
        .filter(api_token::Column::Token.eq(token_str))
        .exec(state.db.as_ref())
        .await?;

    Ok(Json(serde_json::json!({"success": true})))
}
