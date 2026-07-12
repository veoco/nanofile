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
use serde::Deserialize;
use std::sync::Arc;

use crate::AppState;
use crate::auth::password::verify_password;
use crate::auth::totp::TotpManager;
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
    /// Raw backup codes (shown once after generation).
    pub backup_codes: Option<Vec<String>>,
    pub error: Option<String>,
    pub success: Option<String>,
    /// CSRF token for the disable form.
    pub csrf_token: Option<String>,
    pub left_panel_repos: Vec<crate::repo::LeftPanelRepo>,
    pub current_repo_id: Option<String>,
}

// ─── Request types ──────────────────────────────────────────────────────────

#[derive(Deserialize)]
pub struct VerifyForm {
    pub code: String,
    pub csrf_token: Option<String>,
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
    let two_fa = state.repos.user_2fa.find_by_user_id(user.user_id).await?;
    let enabled = two_fa.as_ref().map(|tf| tf.enabled).unwrap_or(false);
    let setup_pending = two_fa.is_some() && !enabled;

    let secret = if setup_pending {
        let tf = two_fa.as_ref().unwrap();
        Some(tf.totp_secret.clone())
    } else {
        None
    };

    let csrf_token = Some(crate::auth::csrf::generate_csrf_token(
        &state.csrf_secret,
        &user.session_token,
    ));

    let left_panel_repos = crate::repo::load_left_panel_repos(&state.repos, user.user_id).await?;
    let tpl = TwoFactorTemplate {
        urls: crate::static_assets::template_urls(),
        user_email: user.email.clone(),
        is_admin: user.is_admin,
        active_page: "settings",
        enabled,
        setup_pending,
        secret,
        backup_codes,
        error,
        success,
        csrf_token,
        left_panel_repos,
        current_repo_id: None,
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
    // Check if already enabled — if so, just show the enabled page.
    let existing = state.repos.user_2fa.find_by_user_id(user.user_id).await?;
    if existing.as_ref().is_some_and(|tf| tf.enabled) {
        return render_page(&user, &state, None, None, None).await;
    }

    // No record or setup-pending → regenerate a fresh secret every time.
    // Delete any old record so we start clean.
    state.repos.user_2fa.delete_by_user_id(user.user_id).await?;

    crate::auth::backup_codes::BackupCodeManager::delete_all_for_user(&state.repos, user.user_id)
        .await?;

    TotpManager::get_or_create_2fa(&state.repos, user.user_id).await?;

    // Generate backup codes for the post-verify display
    let raw_codes = crate::auth::backup_codes::BackupCodeManager::generate_codes(10);
    crate::auth::backup_codes::BackupCodeManager::store_codes(
        &state.repos,
        user.user_id,
        &raw_codes,
    )
    .await?;

    render_page(
        &user,
        &state,
        None,
        Some(
            "Scan the QR code with your authenticator app, then enter the verification code below to enable.".to_string(),
        ),
        None, // backup codes shown only after successful verification
    )
    .await
}

/// POST /profile/two-factor/setup — generate TOTP secret and backup codes.
pub async fn setup_2fa(
    user: WebUser,
    State(state): State<Arc<AppState>>,
    Form(form): Form<std::collections::HashMap<String, String>>,
) -> Result<impl IntoResponse, AppError> {
    crate::auth::csrf::check_form_csrf(
        &state,
        &user.session_token,
        form.get("csrf_token").map(|s| s.as_str()),
    )?;

    TotpManager::get_or_create_2fa(&state.repos, user.user_id).await?;

    // Regenerate backup codes
    crate::auth::backup_codes::BackupCodeManager::delete_all_for_user(&state.repos, user.user_id)
        .await?;

    let raw_codes = crate::auth::backup_codes::BackupCodeManager::generate_codes(10);
    crate::auth::backup_codes::BackupCodeManager::store_codes(
        &state.repos,
        user.user_id,
        &raw_codes,
    )
    .await?;

    render_page(
        &user,
        &state,
        None,
        Some(
            "Scan the QR code with your authenticator app, then enter the verification code below to enable.".to_string(),
        ),
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
    crate::auth::csrf::check_form_csrf(&state, &user.session_token, form.csrf_token.as_deref())?;

    let two_fa = state
        .repos
        .user_2fa
        .find_by_user_id(user.user_id)
        .await?
        .ok_or_else(|| AppError::BadRequest("2FA not set up. Click 'Set Up' first.".into()))?;

    let totp = TotpManager::create_totp(&two_fa.totp_secret, &user.email, "Nanofile")
        .map_err(|e| AppError::internal(e.to_string()))?;

    let code_valid = TotpManager::verify_code(&totp, &form.code);
    let backup_valid = if !code_valid {
        crate::auth::backup_codes::BackupCodeManager::verify_code(
            &state.repos,
            user.user_id,
            &form.code,
        )
        .await
        .unwrap_or(false)
    } else {
        false
    };

    if code_valid || backup_valid {
        let now = chrono::Utc::now().timestamp();
        state
            .repos
            .user_2fa
            .set_enabled(user.user_id, true, now)
            .await?;

        // Generate a fresh set of backup codes for the user to save one last time
        crate::auth::backup_codes::BackupCodeManager::delete_all_for_user(
            &state.repos,
            user.user_id,
        )
        .await?;
        let fresh_codes = crate::auth::backup_codes::BackupCodeManager::generate_codes(10);
        crate::auth::backup_codes::BackupCodeManager::store_codes(
            &state.repos,
            user.user_id,
            &fresh_codes,
        )
        .await?;

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

    let user_record = state
        .repos
        .user
        .find_by_id(user.user_id)
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
    state
        .repos
        .user_2fa
        .find_by_user_id(user.user_id)
        .await?
        .ok_or_else(|| AppError::BadRequest("2FA is not set up.".into()))?;

    state.repos.user_2fa.delete_by_user_id(user.user_id).await?;

    // Also clean up backup codes
    crate::auth::backup_codes::BackupCodeManager::delete_all_for_user(&state.repos, user.user_id)
        .await?;

    Ok((StatusCode::FOUND, [("Location", "/settings/")]).into_response())
}

/// GET /settings/two-factor/qr-code/ — serve QR code generated locally.
pub async fn qr_code_image(
    user: WebUser,
    State(state): State<Arc<AppState>>,
) -> Result<impl IntoResponse, AppError> {
    let two_fa = state
        .repos
        .user_2fa
        .find_by_user_id(user.user_id)
        .await?
        .ok_or_else(|| AppError::BadRequest("2FA not set up.".into()))?;

    let totp = TotpManager::create_totp(&two_fa.totp_secret, &user.email, "Nanofile")
        .map_err(|e| AppError::internal(e.to_string()))?;
    let url = TotpManager::get_otpauth_url(&totp);

    let qr = qrcode::QrCode::new(url.as_bytes()).map_err(|e| AppError::internal(e.to_string()))?;
    let svg = qr
        .render::<qrcode::render::svg::Color>()
        .min_dimensions(200, 200)
        .dark_color(qrcode::render::svg::Color("#000000"))
        .light_color(qrcode::render::svg::Color("#ffffff"))
        .build();

    Ok(([("Content-Type", "image/svg+xml; charset=utf-8")], svg))
}
