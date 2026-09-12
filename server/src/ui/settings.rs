/// Web UI settings handlers — account info, password change, devices.
use askama::Template;
use axum::{
    Form,
    extract::{Multipart, State},
    http::StatusCode,
    response::{Html, IntoResponse, Response},
};
use serde::Deserialize;
use std::sync::Arc;

use crate::AppState;
use crate::handler::read_multipart_field_limited;
use crate::i18n::I18n;
use crate::service::auth::password::{hash_password_async, verify_password_async};
use crate::service::credential::{CredentialKind, CredentialService};
use crate::service::user::avatar::MAX_AVATAR_SIZE;
use base::error::AppError;

use super::auth_extractor::WebUser;

#[derive(Template)]
#[template(path = "settings/index.html")]
pub struct SettingsTemplate {
    pub urls: &'static crate::static_assets::TemplateUrls,
    pub t: &'static I18n,
    pub user_language: &'static str,
    pub user_email: String,
    pub user_display_name: String,
    pub error: Option<String>,
    pub success: Option<String>,
    pub active_page: &'static str,
    /// Whether 2FA is enabled (for status display on settings page).
    pub two_fa_enabled: bool,
    /// CSRF token for form protection.
    pub csrf_token: Option<String>,
    /// Whether the user has admin privileges.
    pub is_admin: bool,
    pub left_panel_repos: Vec<crate::service::repo::service::LeftPanelRepo>,
    pub current_repo_id: Option<String>,
}

#[derive(Template)]
#[template(path = "settings/devices.html")]
pub struct DevicesTemplate {
    pub urls: &'static crate::static_assets::TemplateUrls,
    pub t: &'static I18n,
    pub user_email: String,
    pub is_admin: bool,
    pub active_page: &'static str,
    /// The account's whole credential inventory, already formatted for display.
    pub inventory: InventoryInfo,
    pub error: Option<String>,
    pub success: Option<String>,
    pub csrf_token: Option<String>,
    pub left_panel_repos: Vec<crate::service::repo::service::LeftPanelRepo>,
    pub current_repo_id: Option<String>,
}

/// The inventory with every date and label resolved.
///
/// Formatting lives here rather than in the service because the service has no
/// business knowing about the reader's locale, and askama cannot call into
/// arbitrary helpers.
#[derive(Default)]
pub struct InventoryInfo {
    pub clients: Vec<ClientInfo>,
    pub browsers: Vec<BrowserInfo>,
    pub sync_tokens: Vec<SyncTokenInfo>,
    pub device_trusts: Vec<DeviceTrustInfo>,
    pub api_key_count: usize,
}

pub struct ClientInfo {
    pub platform: String,
    pub platform_display: String,
    pub device_id: String,
    pub device_name: String,
    pub client_version: String,
    pub signed_in: String,
    pub signed_in_ts: i64,
    pub is_desktop_client: bool,
}

pub struct BrowserInfo {
    pub id: i32,
    /// i18n key naming the login this session came from.
    pub source_label: &'static str,
    /// `Chrome · macOS`, or empty when the login carried no usable agent.
    pub client: String,
    pub signed_in: String,
    pub signed_in_ts: i64,
    pub is_current: bool,
}

pub struct SyncTokenInfo {
    pub id: i32,
    /// `None` when the library is gone, so the template can say so.
    pub repo_name: Option<String>,
    pub device_name: Option<String>,
    pub peer_ip: Option<String>,
    pub client_version: Option<String>,
    pub created: String,
    pub created_ts: i64,
    pub last_sync: Option<String>,
    pub expires: String,
}

pub struct DeviceTrustInfo {
    pub id: i32,
    pub device_name: Option<String>,
    pub device_id: Option<String>,
    pub created: String,
    pub created_ts: i64,
    pub expires: String,
}

#[derive(Deserialize)]
pub struct PasswordForm {
    pub old_password: String,
    pub new_password: String,
    pub csrf_token: Option<String>,
}

#[derive(Deserialize)]
pub struct UnlinkDeviceForm {
    pub platform: String,
    pub device_id: String,
    pub csrf_token: Option<String>,
}

#[derive(Deserialize)]
pub struct RevokeCredentialForm {
    /// Which list the id belongs to; see [`CredentialKind`].
    pub kind: String,
    pub id: i32,
    pub csrf_token: Option<String>,
}

#[derive(Deserialize)]
pub struct DisplayNameForm {
    pub display_name: String,
    pub csrf_token: Option<String>,
}

#[derive(Deserialize)]
pub struct LanguageForm {
    pub language: String,
    pub csrf_token: Option<String>,
}

/// GET /profile/ — account settings page.
pub async fn settings_page(
    user: WebUser,
    State(state): State<Arc<AppState>>,
) -> Result<Html<String>, AppError> {
    let user_record = state
        .repos
        .user
        .find_by_id(user.user_id)
        .await?
        .ok_or_else(|| AppError::NotFound("User not found".to_string()))?;

    let two_fa = state.repos.user_2fa.find_by_user_id(user.user_id).await?;
    let two_fa_enabled = two_fa.as_ref().map(|tf| tf.enabled).unwrap_or(false);

    let ctx = crate::ui::ctx::build_page_ctx(&state, &user).await?;
    let tpl = SettingsTemplate {
        urls: ctx.urls,
        t: ctx.t,
        user_language: ctx.t.lang,
        user_email: ctx.user_email,
        user_display_name: user_record.nickname(),
        error: None,
        success: None,
        active_page: "settings",
        two_fa_enabled,
        csrf_token: Some(ctx.csrf_token),
        is_admin: ctx.is_admin,
        left_panel_repos: ctx.left_panel_repos,
        current_repo_id: None,
    };

    let html = tpl
        .render()
        .map_err(|e| AppError::internal(e.to_string()))?;
    Ok(Html(html))
}

/// POST /profile/password — change password.
pub async fn change_password(
    user: WebUser,
    State(state): State<Arc<AppState>>,
    Form(form): Form<PasswordForm>,
) -> Result<impl IntoResponse, AppError> {
    // CSRF check — the token is mandatory; a missing token is rejected.
    crate::service::auth::csrf::check_form_csrf(
        &state,
        &user.session_token,
        form.csrf_token.as_deref(),
    )?;

    let user_record = state
        .repos
        .user
        .find_by_id(user.user_id)
        .await
        .map_err(|e| AppError::internal(format!("db error: {e}")))?
        .ok_or_else(|| AppError::NotFound("User not found".to_string()))?;

    if !verify_password_async(
        form.old_password.clone(),
        user_record.password_hash.clone(),
        state.config.auth.password_hash_iterations,
    )
    .await
    {
        return render_settings_error(
            &state,
            &user,
            Some(
                crate::ui::ctx::build_page_ctx(&state, &user)
                    .await?
                    .t
                    .tr("settings.incorrect_password")
                    .to_string(),
            ),
        )
        .await;
    }

    // Enforce the configured password policy. The self-service change form
    // previously bypassed it entirely (a one-character password was accepted),
    // unlike registration and password reset.
    if let Err(msg) = crate::service::auth::password::validate_password(
        &form.new_password,
        state.config.auth.password_min_length,
        state.config.auth.require_strong_password,
    ) {
        return render_settings_error(&state, &user, Some(msg)).await;
    }

    let new_hash = hash_password_async(
        form.new_password.clone(),
        state.config.auth.password_hash_iterations,
    )
    .await;
    state
        .repos
        .user
        .update_password(user.user_id, new_hash)
        .await
        .map_err(|e| AppError::internal(format!("update failed: {e}")))?;

    // Revoke every other credential — account API tokens, 2FA device-trust
    // tokens and all repository sync tokens — while keeping the acting session
    // alive. Mirrors seahub's `clear_token()` + `update_session_auth_hash()`.
    crate::service::auth::token::revoke_all_credentials(
        &state.repos,
        Some(&state.token_manager),
        user.user_id,
        Some(user.session_token.as_str()),
    )
    .await?;

    Ok((StatusCode::FOUND, [("Location", "/settings/")]).into_response())
}

/// POST /profile/display-name — update the user's display name.
pub async fn update_display_name(
    user: WebUser,
    State(state): State<Arc<AppState>>,
    Form(form): Form<DisplayNameForm>,
) -> Result<impl IntoResponse, AppError> {
    // CSRF check — the token is mandatory; a missing token is rejected.
    crate::service::auth::csrf::check_form_csrf(
        &state,
        &user.session_token,
        form.csrf_token.as_deref(),
    )?;

    let display_name = if form.display_name.trim().is_empty() {
        None
    } else {
        Some(form.display_name.trim().to_string())
    };
    state
        .repos
        .user
        .update_display_name(user.user_id, display_name)
        .await?;

    Ok((StatusCode::FOUND, [("Location", "/settings/")]).into_response())
}

/// POST /settings/language/ — update the user's preferred UI language.
pub async fn update_language(
    user: WebUser,
    State(state): State<Arc<AppState>>,
    Form(form): Form<LanguageForm>,
) -> Result<impl IntoResponse, AppError> {
    crate::service::auth::csrf::check_form_csrf(
        &state,
        &user.session_token,
        form.csrf_token.as_deref(),
    )?;

    let normalized = I18n::normalize_lang(&form.language)
        .ok_or_else(|| AppError::BadRequest("Unsupported language.".to_string()))?;
    state
        .repos
        .user
        .update_language(user.user_id, Some(normalized.to_string()))
        .await?;

    Ok((StatusCode::FOUND, [("Location", "/settings/")]).into_response())
}

/// GET /settings/devices/ — the account's credential inventory.
///
/// Everything long-lived the account holds, in one place: client sessions,
/// browser sessions, repository sync tokens, 2FA device trusts, and a pointer
/// to the API keys. The page exists because a credential the owner cannot see
/// is one they cannot revoke — which is how sync tokens, with a one-year
/// default lifetime, used to be.
pub async fn devices_page(
    user: WebUser,
    State(state): State<Arc<AppState>>,
) -> Result<Html<String>, AppError> {
    let service = CredentialService::new(state.repos.clone());
    let inventory = service
        .inventory(user.user_id, Some(user.session_id))
        .await?;

    let info = InventoryInfo {
        clients: inventory
            .clients
            .into_iter()
            .map(|client| ClientInfo {
                platform: client.platform,
                platform_display: client.platform_display,
                device_id: client.device_id,
                device_name: client.device_name,
                client_version: client.client_version,
                signed_in: super::format_ts(client.created_ts),
                signed_in_ts: client.created_ts,
                is_desktop_client: client.is_desktop_client,
            })
            .collect(),
        browsers: inventory
            .browsers
            .into_iter()
            .map(|browser| BrowserInfo {
                id: browser.id,
                source_label: match browser.source {
                    crate::domain::session_source::SessionSource::WebClientLogin => {
                        "credential.source_web_client_login"
                    }
                    _ => "credential.source_web",
                },
                client: browser.client,
                signed_in: super::format_ts(browser.created_ts),
                signed_in_ts: browser.created_ts,
                is_current: browser.is_current,
            })
            .collect(),
        sync_tokens: inventory
            .sync_tokens
            .into_iter()
            .map(|token| SyncTokenInfo {
                id: token.id,
                repo_name: token.repo_name,
                device_name: token.device_name,
                peer_ip: token.peer_ip,
                client_version: token.client_version,
                created: super::format_ts(token.created_ts),
                created_ts: token.created_ts,
                last_sync: token.last_sync_ts.map(super::format_ts),
                expires: super::format_ts_opt(
                    I18n::get(user.language.as_deref()),
                    token.expires_at,
                ),
            })
            .collect(),
        device_trusts: inventory
            .device_trusts
            .into_iter()
            .map(|trust| DeviceTrustInfo {
                id: trust.id,
                device_name: trust.device_name,
                device_id: trust.device_id,
                created: super::format_ts(trust.created_ts),
                created_ts: trust.created_ts,
                expires: super::format_ts(trust.expires_at),
            })
            .collect(),
        api_key_count: inventory.api_key_count,
    };

    let csrf_token = Some(crate::service::auth::csrf::generate_csrf_token(
        &state.csrf_secret,
        &user.session_token,
    ));
    let left_panel_repos = state
        .left_panel_cache
        .get_for_user(&state.repos, user.user_id)
        .await?;

    let tpl = DevicesTemplate {
        urls: crate::static_assets::template_urls(),
        t: I18n::get(user.language.as_deref()),
        user_email: user.email,
        is_admin: user.is_admin,
        active_page: "settings",
        inventory: info,
        error: None,
        success: None,
        csrf_token,
        left_panel_repos,
        current_repo_id: None,
    };

    let html = tpl
        .render()
        .map_err(|e| AppError::internal(e.to_string()))?;
    Ok(Html(html))
}

/// POST /settings/devices/revoke/ — revoke one credential by kind and id.
///
/// A device is not revoked here: [`unlink_device`] owns that, because one
/// device holds several credentials that have to go together.
pub async fn revoke_credential(
    user: WebUser,
    State(state): State<Arc<AppState>>,
    Form(form): Form<RevokeCredentialForm>,
) -> Result<impl IntoResponse, AppError> {
    crate::service::auth::csrf::check_form_csrf(
        &state,
        &user.session_token,
        form.csrf_token.as_deref(),
    )?;

    let kind = CredentialKind::from_id(&form.kind)
        .ok_or_else(|| AppError::BadRequest("unknown credential kind".into()))?;

    // Revoking the session you are reading this page with is allowed -- it is
    // how you sign yourself out remotely -- but it has to leave you at the
    // login page rather than on a page you can no longer load.
    let signing_out_self = kind == CredentialKind::BrowserSession && form.id == user.session_id;

    let service = CredentialService::new(state.repos.clone());
    service.revoke(user.user_id, kind, form.id).await?;

    let location = if signing_out_self {
        "/accounts/login/"
    } else {
        "/settings/devices/"
    };
    Ok((StatusCode::FOUND, [("Location", location)]).into_response())
}

/// POST /profile/devices/ — remove a device's tokens (API, S2FA, sync).
pub async fn unlink_device(
    user: WebUser,
    State(state): State<Arc<AppState>>,
    Form(form): Form<UnlinkDeviceForm>,
) -> Result<impl IntoResponse, AppError> {
    crate::service::auth::csrf::check_form_csrf(
        &state,
        &user.session_token,
        form.csrf_token.as_deref(),
    )?;

    // 1. Delete API tokens for this device (identified by platform + device_id).
    state
        .repos
        .api_token
        .delete_many_by_user_platform_device(user.user_id, &form.platform, &form.device_id)
        .await?;

    // 2. Delete S2FA device trust tokens (identified by device_id).
    state
        .repos
        .s2fa_token
        .delete_by_user_and_device(user.user_id, &form.device_id)
        .await?;

    // 3. Delete sync tokens linked to this device via peer_id (= client_id).
    state
        .repos
        .sync_token
        .delete_by_user_and_peer(user.user_id, &form.device_id)
        .await?;

    Ok((StatusCode::FOUND, [("Location", "/settings/devices/")]).into_response())
}

// ─── Avatar upload (web UI) ──────────────────────────────────────────────────

/// POST /profile/avatar — upload a new avatar from the web UI.
pub async fn upload_avatar(
    user: WebUser,
    State(state): State<Arc<AppState>>,
    mut multipart: Multipart,
) -> Result<impl IntoResponse, AppError> {
    // Extract the avatar file from the multipart stream.
    let mut avatar_field: Option<(String, Vec<u8>)> = None;
    let mut csrf_token: Option<String> = None;

    while let Some(mut field) = multipart
        .next_field()
        .await
        .map_err(|e| AppError::BadRequest(e.to_string()))?
    {
        let name = field.name().unwrap_or("").to_string();
        match name.as_str() {
            "csrf_token" => {
                csrf_token = Some(field.text().await.unwrap_or_default());
            }
            "avatar" => {
                let file_name = field.file_name().unwrap_or("avatar.png").to_string();
                let data = match read_multipart_field_limited(&mut field, MAX_AVATAR_SIZE).await {
                    Ok(d) => d,
                    Err(_) => {
                        return render_settings_error(
                            &state,
                            &user,
                            Some(
                                I18n::get(user.language.as_deref())
                                    .tr("settings.upload_avatar_failed")
                                    .to_string(),
                            ),
                        )
                        .await;
                    }
                };
                avatar_field = Some((file_name, data));
            }
            _ => {}
        }
    }

    // CSRF check
    let expected_csrf =
        crate::service::auth::csrf::generate_csrf_token(&state.csrf_secret, &user.session_token);
    if csrf_token.as_deref() != Some(&expected_csrf) {
        return render_settings_error(
            &state,
            &user,
            Some(
                I18n::get(user.language.as_deref())
                    .tr("common.invalid_csrf")
                    .to_string(),
            ),
        )
        .await;
    }

    let (file_name, data) =
        avatar_field.ok_or_else(|| AppError::BadRequest("no avatar file provided".into()))?;

    // Delegate to the shared AvatarService which handles validation (size/ext),
    // persistence, thumbnail generation (with square crop + EXIF), and DB upsert.
    let svc = state.avatar_service();
    match svc.upload_avatar(&user.email, file_name, data).await {
        Ok(_url) => Ok((StatusCode::FOUND, [("Location", "/settings/")]).into_response()),
        Err(e) => {
            let msg = match &e {
                AppError::BadRequest(m) => m.clone(),
                _ => I18n::get(user.language.as_deref())
                    .tr("settings.upload_avatar_failed")
                    .to_string(),
            };
            render_settings_error(&state, &user, Some(msg)).await
        }
    }
}

/// Re-render the settings page with an error message.
async fn render_settings_error(
    state: &Arc<AppState>,
    user: &WebUser,
    error: Option<String>,
) -> Result<Response, AppError> {
    let csrf_new = Some(crate::service::auth::csrf::generate_csrf_token(
        &state.csrf_secret,
        &user.session_token,
    ));
    let left_panel_repos = state
        .left_panel_cache
        .get_for_user(&state.repos, user.user_id)
        .await?;
    let tpl = SettingsTemplate {
        urls: crate::static_assets::template_urls(),
        t: I18n::get(user.language.as_deref()),
        user_language: I18n::get(user.language.as_deref()).lang,
        user_email: user.email.clone(),
        user_display_name: user.email.split('@').next().unwrap_or("").to_string(),
        error,
        success: None,
        active_page: "settings",
        two_fa_enabled: false,
        csrf_token: csrf_new,
        is_admin: user.is_admin,
        left_panel_repos,
        current_repo_id: None,
    };
    let html = tpl
        .render()
        .map_err(|e| AppError::internal(e.to_string()))?;
    Ok((StatusCode::OK, Html(html)).into_response())
}
