use axum::{Json, Router, extract::State};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

use crate::AppState;
use crate::auth::middleware::AuthUser;
use crate::auth::service::two_factor_service::TwoFactorService;
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
    let svc = TwoFactorService::new(
        state.db.clone(),
        state.repos.clone(),
        state.config.auth.password_hash_iterations,
        state.disable_2fa_limiter.clone(),
    );

    let result = svc.setup_2fa(auth.user_id, &auth.email).await?;

    Ok(Json(SetupResponse {
        secret: result.secret,
        otpauth_url: result.otpauth_url,
        backup_codes: result.backup_codes,
    }))
}

pub async fn verify_2fa(
    auth: AuthUser,
    State(state): State<Arc<AppState>>,
    Json(req): Json<VerifyRequest>,
) -> Result<Json<VerifyResponse>, AppError> {
    let svc = TwoFactorService::new(
        state.db.clone(),
        state.repos.clone(),
        state.config.auth.password_hash_iterations,
        state.disable_2fa_limiter.clone(),
    );

    svc.verify_2fa(auth.user_id, &auth.email, &req.code).await?;

    Ok(Json(VerifyResponse { success: true }))
}

pub async fn disable_2fa(
    auth: AuthUser,
    State(state): State<Arc<AppState>>,
    Json(req): Json<DisableRequest>,
) -> Result<Json<VerifyResponse>, AppError> {
    let svc = TwoFactorService::new(
        state.db.clone(),
        state.repos.clone(),
        state.config.auth.password_hash_iterations,
        state.disable_2fa_limiter.clone(),
    );

    svc.disable_2fa(auth.user_id, &req.password).await?;

    Ok(Json(VerifyResponse { success: true }))
}
