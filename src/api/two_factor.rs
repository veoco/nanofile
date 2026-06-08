use axum::{Json, Router, extract::State};
use sea_orm::{ActiveModelTrait, EntityTrait};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

use crate::AppState;
use crate::auth::middleware::AuthUser;
use crate::auth::totp::TotpManager;
use crate::entity::user_2fa;
use crate::error::AppError;

#[derive(Serialize)]
pub struct SetupResponse {
    pub secret: String,
    pub otpauth_url: String,
    pub backup_codes: Vec<String>,
}

#[derive(Deserialize)]
pub struct VerifyRequest {
    pub code: String,
}

#[derive(Serialize)]
pub struct VerifyResponse {
    pub success: bool,
}

#[derive(Deserialize)]
pub struct DisableRequest {
    pub password: String,
}

pub fn two_factor_routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/setup/", axum::routing::post(setup_2fa))
        .route("/verify/", axum::routing::post(verify_2fa))
        .route("/disable/", axum::routing::post(disable_2fa))
}

pub async fn setup_2fa(
    auth: AuthUser,
    State(state): State<Arc<AppState>>,
) -> Result<Json<SetupResponse>, AppError> {
    let model = TotpManager::get_or_create_2fa(state.db.as_ref(), auth.user_id).await?;

    let totp = TotpManager::create_totp(&model.totp_secret, &auth.email, "Nanofile")
        .map_err(|e| AppError::Internal(e.to_string()))?;

    let backup_codes = crate::auth::backup_codes::BackupCodeManager::generate_codes(10);
    crate::auth::backup_codes::BackupCodeManager::store_codes(
        state.db.as_ref(),
        auth.user_id,
        &backup_codes,
    )
    .await
    .map_err(|e| AppError::Internal(e.to_string()))?;

    Ok(Json(SetupResponse {
        secret: model.totp_secret,
        otpauth_url: TotpManager::get_otpauth_url(&totp),
        backup_codes,
    }))
}

pub async fn verify_2fa(
    auth: AuthUser,
    State(state): State<Arc<AppState>>,
    Json(req): Json<VerifyRequest>,
) -> Result<Json<VerifyResponse>, AppError> {
    let model = user_2fa::Entity::find_by_id(auth.user_id)
        .one(state.db.as_ref())
        .await?
        .ok_or_else(|| AppError::BadRequest("2FA not set up".into()))?;

    let totp = TotpManager::create_totp(&model.totp_secret, &auth.email, "Nanofile")
        .map_err(|e| AppError::Internal(e.to_string()))?;

    if TotpManager::verify_code(&totp, &req.code) {
        let now = chrono::Utc::now().timestamp();
        let mut active: user_2fa::ActiveModel = model.into();
        active.enabled = sea_orm::Set(true);
        active.enabled_at = sea_orm::Set(Some(now));
        active.update(state.db.as_ref()).await?;

        Ok(Json(VerifyResponse { success: true }))
    } else {
        Err(AppError::TwoFactorInvalid)
    }
}

pub async fn disable_2fa(
    auth: AuthUser,
    State(state): State<Arc<AppState>>,
    Json(req): Json<DisableRequest>,
) -> Result<Json<VerifyResponse>, AppError> {
    let user = crate::entity::user::Entity::find_by_id(auth.user_id)
        .one(state.db.as_ref())
        .await?
        .ok_or(AppError::Unauthorized)?;

    if !crate::auth::password::verify_password(
        &req.password,
        &user.password_hash,
        state.config.auth.password_hash_iterations,
    ) {
        return Err(AppError::Unauthorized);
    }

    let model = user_2fa::Entity::find_by_id(auth.user_id)
        .one(state.db.as_ref())
        .await?
        .ok_or_else(|| AppError::BadRequest("2FA not set up".into()))?;

    let mut active: user_2fa::ActiveModel = model.into();
    active.enabled = sea_orm::Set(false);
    active.update(state.db.as_ref()).await?;

    Ok(Json(VerifyResponse { success: true }))
}
