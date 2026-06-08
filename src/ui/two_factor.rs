/// Web UI two-factor authentication handlers.
///
/// GET /profile/two-factor/ — dedicated 2FA management page.
/// POST /profile/two-factor/setup  — generate secret + backup codes.
/// POST /profile/two-factor/verify — verify TOTP code and enable.
/// POST /profile/two-factor/disable — disable 2FA (requires password).
use askama::Template;
use axum::{
    Form,
    extract::State,
    http::StatusCode,
    response::{Html, IntoResponse},
};
use sea_orm::{ActiveModelTrait, ColumnTrait, EntityTrait, QueryFilter, Set};
use serde::Deserialize;
use std::sync::Arc;

use crate::AppState;
use crate::auth::password::verify_password;
use crate::auth::totp::TotpManager;
use crate::entity::user_2fa;
use crate::error::AppError;

use super::auth_extractor::WebUser;

// ─── Templates ───────────────────────────────────────────────────────────────

#[derive(Template)]
#[template(path = "settings/two_factor.html")]
pub struct TwoFactorTemplate {
    pub urls: &'static crate::static_assets::TemplateUrls,
    pub user_email: String,
    pub is_admin: bool,
    pub active_page: &'static str,
    /// Whether 2FA is currently enabled.
    pub enabled: bool,
    /// Whether setup is in progress (secret exists but not yet verified).
    pub setup_pending: bool,
    /// TOTP secret (shown during setup).
    pub secret: Option<String>,
    /// otpauth:// URL for QR code.
    pub otpauth_url: Option<String>,
    /// Raw backup codes (shown once after generation).
    pub backup_codes: Option<Vec<String>>,
    pub error: Option<String>,
    pub success: Option<String>,
    /// CSRF token for the disable form.
    pub csrf_token: Option<String>,
}

// ─── Request types ──────────────────────────────────────────────────────────

#[derive(Deserialize)]
pub struct VerifyForm {
    pub code: String,
}

#[derive(Deserialize)]
pub struct DisableForm {
    pub password: String,
    pub csrf_token: Option<String>,
}

// ─── Helpers ────────────────────────────────────────────────────────────────

async fn render_page(
    user: &WebUser,
    state: &Arc<AppState>,
    error: Option<String>,
    success: Option<String>,
    backup_codes: Option<Vec<String>>,
) -> Result<Html<String>, AppError> {
    let db = state.db.as_ref();
    let two_fa = user_2fa::Entity::find_by_id(user.user_id).one(db).await?;
    let enabled = two_fa.as_ref().map(|tf| tf.enabled).unwrap_or(false);
    let setup_pending = two_fa.is_some() && !enabled;

    let (secret, otpauth_url) = if setup_pending {
        let tf = two_fa.as_ref().unwrap();
        let totp = TotpManager::create_totp(&tf.totp_secret, &user.email, "Nanofile")
            .map_err(|e| AppError::internal(e.to_string()))?;
        (
            Some(tf.totp_secret.clone()),
            Some(TotpManager::get_otpauth_url(&totp)),
        )
    } else {
        (None, None)
    };

    let csrf_token = Some(crate::auth::csrf::generate_csrf_token(
        &state.csrf_secret,
        &user.session_token,
    ));

    let tpl = TwoFactorTemplate {
        urls: crate::static_assets::template_urls(),
        user_email: user.email.clone(),
        is_admin: user.is_admin,
        active_page: "settings",
        enabled,
        setup_pending,
        secret,
        otpauth_url,
        backup_codes,
        error,
        success,
        csrf_token,
    };

    let html = tpl
        .render()
        .map_err(|e| AppError::internal(e.to_string()))?;
    Ok(Html(html))
}

// ─── Handlers ───────────────────────────────────────────────────────────────

/// GET /profile/two-factor/ — show the 2FA management page.
///
/// Every visit (including refresh) generates a **fresh** secret and QR code
/// when still in setup-pending state, so a leaked page snapshot is worthless.
pub async fn setup_page(
    user: WebUser,
    State(state): State<Arc<AppState>>,
) -> Result<Html<String>, AppError> {
    let db = state.db.as_ref();

    // Check if already enabled — if so, just show the enabled page.
    let existing = user_2fa::Entity::find_by_id(user.user_id).one(db).await?;
    if existing.as_ref().is_some_and(|tf| tf.enabled) {
        return render_page(&user, &state, None, None, None).await;
    }

    // No record or setup-pending → regenerate a fresh secret every time.
    // Delete any old record so we start clean.
    user_2fa::Entity::delete_many()
        .filter(user_2fa::Column::UserId.eq(user.user_id))
        .exec(db)
        .await?;

    crate::auth::backup_codes::BackupCodeManager::delete_all_for_user(db, user.user_id)
        .await
        .map_err(|e| AppError::internal(e.to_string()))?;

    TotpManager::get_or_create_2fa(db, user.user_id)
        .await
        .map_err(|e| AppError::internal(e.to_string()))?;

    // Generate backup codes for the post-verify display
    let raw_codes = crate::auth::backup_codes::BackupCodeManager::generate_codes(10);
    crate::auth::backup_codes::BackupCodeManager::store_codes(db, user.user_id, &raw_codes)
        .await
        .map_err(|e| AppError::internal(e.to_string()))?;

    render_page(&user, &state,
        None,
        Some("Scan the QR code with your authenticator app, then enter the verification code below to enable.".to_string()),
        None, // backup codes shown only after successful verification
    )
    .await
}

/// POST /profile/two-factor/setup — generate TOTP secret and backup codes.
pub async fn setup_2fa(
    user: WebUser,
    State(state): State<Arc<AppState>>,
) -> Result<impl IntoResponse, AppError> {
    let db = state.db.as_ref();

    TotpManager::get_or_create_2fa(db, user.user_id)
        .await
        .map_err(|e| AppError::internal(e.to_string()))?;

    // Regenerate backup codes
    crate::auth::backup_codes::BackupCodeManager::delete_all_for_user(db, user.user_id)
        .await
        .map_err(|e| AppError::internal(e.to_string()))?;

    let raw_codes = crate::auth::backup_codes::BackupCodeManager::generate_codes(10);
    crate::auth::backup_codes::BackupCodeManager::store_codes(db, user.user_id, &raw_codes)
        .await
        .map_err(|e| AppError::internal(e.to_string()))?;

    render_page(&user, &state,
        None,
        Some("Scan the QR code with your authenticator app, then enter the verification code below to enable.".to_string()),
        Some(raw_codes),
    )
    .await
    .map(|html| (StatusCode::OK, html).into_response())
}

/// POST /profile/two-factor/verify — verify TOTP code and enable 2FA.
pub async fn verify_2fa(
    user: WebUser,
    State(state): State<Arc<AppState>>,
    Form(form): Form<VerifyForm>,
) -> Result<impl IntoResponse, AppError> {
    let db = state.db.as_ref();

    let two_fa = user_2fa::Entity::find_by_id(user.user_id)
        .one(db)
        .await?
        .ok_or_else(|| AppError::BadRequest("2FA not set up. Click 'Set Up' first.".into()))?;

    let totp = TotpManager::create_totp(&two_fa.totp_secret, &user.email, "Nanofile")
        .map_err(|e| AppError::internal(e.to_string()))?;

    let code_valid = TotpManager::verify_code(&totp, &form.code);
    let backup_valid = if !code_valid {
        crate::auth::backup_codes::BackupCodeManager::verify_code(db, user.user_id, &form.code)
            .await
            .unwrap_or(false)
    } else {
        false
    };

    if code_valid || backup_valid {
        let now = chrono::Utc::now().timestamp();
        let mut active: user_2fa::ActiveModel = two_fa.into();
        active.enabled = Set(true);
        active.enabled_at = Set(Some(now));
        active.update(db).await?;

        // Generate a fresh set of backup codes for the user to save one last time
        crate::auth::backup_codes::BackupCodeManager::delete_all_for_user(db, user.user_id)
            .await
            .map_err(|e| AppError::internal(e.to_string()))?;
        let fresh_codes = crate::auth::backup_codes::BackupCodeManager::generate_codes(10);
        crate::auth::backup_codes::BackupCodeManager::store_codes(db, user.user_id, &fresh_codes)
            .await
            .map_err(|e| AppError::internal(e.to_string()))?;

        render_page(
            &user,
            &state,
            None,
            Some("Two-factor authentication is now enabled. Save your backup codes below \u{2014} they will not be shown again.".to_string()),
            Some(fresh_codes),
        )
        .await
        .map(|html| (StatusCode::OK, html).into_response())
    } else {
        render_page(
            &user,
            &state,
            Some("Invalid verification code. Please try again.".to_string()),
            None,
            None,
        )
        .await
        .map(|html| (StatusCode::OK, html).into_response())
    }
}

/// POST /profile/two-factor/disable — disable 2FA (requires password).
pub async fn disable_2fa(
    user: WebUser,
    State(state): State<Arc<AppState>>,
    Form(form): Form<DisableForm>,
) -> Result<impl IntoResponse, AppError> {
    // CSRF check — only validate when form includes a token (gradual rollout).
    if let Some(ref token) = form.csrf_token {
        let expected =
            crate::auth::csrf::generate_csrf_token(&state.csrf_secret, &user.session_token);
        if *token != expected {
            return Err(AppError::BadRequest("Invalid CSRF token.".to_string()));
        }
    }

    let db = state.db.as_ref();

    let user_record = crate::entity::user::Entity::find_by_id(user.user_id)
        .one(db)
        .await?
        .ok_or(AppError::Unauthorized)?;

    if !verify_password(
        &form.password,
        &user_record.password_hash,
        state.config.auth.password_hash_iterations,
    ) {
        return render_page(
            &user,
            &state,
            Some("Incorrect password.".to_string()),
            None,
            None,
        )
        .await
        .map(|html| (StatusCode::OK, html).into_response());
    }

    // Verify 2FA is set up, then delete for a fresh secret on next setup
    user_2fa::Entity::find_by_id(user.user_id)
        .one(db)
        .await?
        .ok_or_else(|| AppError::BadRequest("2FA is not set up.".into()))?;

    user_2fa::Entity::delete_many()
        .filter(user_2fa::Column::UserId.eq(user.user_id))
        .exec(db)
        .await?;

    // Also clean up backup codes
    crate::auth::backup_codes::BackupCodeManager::delete_all_for_user(db, user.user_id)
        .await
        .map_err(|e| AppError::internal(e.to_string()))?;

    Ok((StatusCode::FOUND, [("Location", "/profile/")]).into_response())
}
