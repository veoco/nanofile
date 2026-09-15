/// Web UI settings handlers — the account, its security, and its credentials.
///
/// The settings area is one shell with a shared navigation. Each page owns one
/// subject: `/settings/` summarises, `/settings/profile/` holds the identity
/// fields, `/settings/security/` the password and two-factor state, and
/// `/settings/credentials/` the session and credential inventory. The old
/// flat card grid mixed all of them, which is why nothing could be found and
/// why the credential page had no way in.
use askama::Template;
use axum::{
    Form,
    extract::{Multipart, Query, State},
    http::StatusCode,
    response::{Html, IntoResponse, Response},
};
use serde::Deserialize;
use std::sync::Arc;

use crate::AppState;
use crate::handler::read_multipart_field_limited;
use crate::i18n::I18n;
use crate::service::auth::password::{hash_password_async, verify_password_async};
use crate::service::credential::{BulkRevoke, CredentialKind, CredentialService};
use crate::service::user::avatar::MAX_AVATAR_SIZE;
use base::error::AppError;

use super::auth_extractor::WebUser;

// ─── Shared settings navigation ─────────────────────────────────────────────

/// What the settings sidebar needs to render itself.
///
/// Built by every settings page so the navigation highlights the current page
/// and carries live counts (an out-of-date "3 credentials" badge is worse than
/// no badge).
pub struct SettingsNav {
    pub active: &'static str,
    pub is_admin: bool,
    pub credential_count: usize,
    pub api_key_count: usize,
    pub two_fa_enabled: bool,
}

impl SettingsNav {
    /// The page key used to highlight the "Overview" entry.
    pub const OVERVIEW: &'static str = "settings";
    pub const PROFILE: &'static str = "settings.profile";
    pub const SECURITY: &'static str = "settings.security";
    pub const CREDENTIALS: &'static str = "settings.credentials";
    pub const API_KEYS: &'static str = "settings.api_keys";
    pub const INVITATIONS: &'static str = "settings.invitations";
}

/// Collect the sidebar's live state for `user`.
///
/// Public because the settings pages outside this module (API keys, invitation
/// codes, two-factor setup) render the same navigation.
pub async fn build_nav(
    state: &AppState,
    user: &WebUser,
    active: &'static str,
) -> Result<SettingsNav, AppError> {
    let service = CredentialService::new(state.repos.clone());
    let inventory = service
        .inventory(user.user_id, Some(user.session_id))
        .await?;
    let two_fa = state.repos.user_2fa.find_by_user_id(user.user_id).await?;
    Ok(SettingsNav {
        active,
        is_admin: user.is_admin,
        // Every credential row that can reach the account, counted one by one:
        // a device contributes its session plus each token and trust it holds.
        credential_count: inventory.browsers.len()
            + inventory.clients.len()
            + inventory.sync_tokens.len()
            + inventory.device_trusts.len(),
        api_key_count: inventory.api_key_count,
        two_fa_enabled: two_fa.as_ref().map(|tf| tf.enabled).unwrap_or(false),
    })
}

// ─── Overview ───────────────────────────────────────────────────────────────

#[derive(Template)]
#[template(path = "settings/index.html")]
pub struct SettingsIndexTemplate {
    pub urls: &'static crate::static_assets::TemplateUrls,
    pub t: &'static I18n,
    pub user_email: String,
    pub user_display_name: String,
    pub is_admin: bool,
    pub active_page: &'static str,
    pub nav: SettingsNav,
    /// The credential summary the overview cards are built from.
    pub summary: crate::service::credential::CredentialSummary,
    pub other_browser_sessions: usize,
    pub user_language: &'static str,
    /// The page no longer posts anything itself, but every authenticated page
    /// carries a CSRF token: `/settings/` is where other forms read one from.
    pub csrf_token: Option<String>,
    /// `expired_sync_tokens` and `other_browser_sessions` rendered for the
    /// warning line. Empty when there is nothing to warn about.
    pub warning_lines: Vec<String>,
    pub left_panel_repos: Vec<crate::service::repo::service::LeftPanelRepo>,
    pub current_repo_id: Option<String>,
}

/// GET /settings/ — the overview: state at a glance and links into each area.
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

    let service = CredentialService::new(state.repos.clone());
    let inventory = service
        .inventory(user.user_id, Some(user.session_id))
        .await?;
    let mut summary = inventory.summary.clone();
    summary.two_fa_enabled = two_fa_enabled;

    let ctx = crate::ui::ctx::build_page_ctx(&state, &user).await?;

    // The overview's one line of warning, assembled here so the template does
    // not have to count things or pluralise them.
    let mut warning_lines: Vec<String> = Vec::new();
    if summary.expired_sync_tokens > 0 {
        warning_lines.push(ctx.t.trf(
            "credential.warn_expired",
            &[("count", summary.expired_sync_tokens.to_string())],
        ));
    }
    if inventory.other_browser_sessions() > 0 {
        warning_lines.push(ctx.t.trf(
            "credential.warn_other_sessions",
            &[("count", inventory.other_browser_sessions().to_string())],
        ));
    }

    let nav = SettingsNav {
        active: SettingsNav::OVERVIEW,
        is_admin: user.is_admin,
        credential_count: inventory.browsers.len()
            + inventory.clients.len()
            + inventory.sync_tokens.len()
            + inventory.device_trusts.len(),
        api_key_count: inventory.api_key_count,
        two_fa_enabled,
    };

    let tpl = SettingsIndexTemplate {
        urls: ctx.urls,
        t: ctx.t,
        user_email: ctx.user_email,
        user_display_name: user_record.nickname(),
        is_admin: ctx.is_admin,
        active_page: SettingsNav::OVERVIEW,
        nav,
        summary,
        other_browser_sessions: inventory.other_browser_sessions(),
        user_language: ctx.t.lang,
        csrf_token: Some(ctx.csrf_token),
        warning_lines,
        left_panel_repos: ctx.left_panel_repos,
        current_repo_id: None,
    };

    let html = tpl
        .render()
        .map_err(|e| AppError::internal(e.to_string()))?;
    Ok(Html(html))
}

// ─── Profile ────────────────────────────────────────────────────────────────

#[derive(Template)]
#[template(path = "settings/profile.html")]
pub struct ProfileTemplate {
    pub urls: &'static crate::static_assets::TemplateUrls,
    pub t: &'static I18n,
    pub user_language: &'static str,
    pub user_email: String,
    pub user_display_name: String,
    pub error: Option<String>,
    pub success: Option<String>,
    pub is_admin: bool,
    pub active_page: &'static str,
    pub nav: SettingsNav,
    pub csrf_token: Option<String>,
    pub left_panel_repos: Vec<crate::service::repo::service::LeftPanelRepo>,
    pub current_repo_id: Option<String>,
}

/// GET /settings/profile/ — avatar, display name and interface language.
pub async fn profile_page(
    user: WebUser,
    State(state): State<Arc<AppState>>,
) -> Result<Html<String>, AppError> {
    let user_record = state
        .repos
        .user
        .find_by_id(user.user_id)
        .await?
        .ok_or_else(|| AppError::NotFound("User not found".to_string()))?;

    let ctx = crate::ui::ctx::build_page_ctx(&state, &user).await?;
    let nav = build_nav(&state, &user, SettingsNav::PROFILE).await?;
    let tpl = ProfileTemplate {
        urls: ctx.urls,
        t: ctx.t,
        user_language: ctx.t.lang,
        user_email: ctx.user_email,
        user_display_name: user_record.nickname(),
        error: None,
        success: None,
        is_admin: ctx.is_admin,
        active_page: SettingsNav::PROFILE,
        nav,
        csrf_token: Some(ctx.csrf_token),
        left_panel_repos: ctx.left_panel_repos,
        current_repo_id: None,
    };

    let html = tpl
        .render()
        .map_err(|e| AppError::internal(e.to_string()))?;
    Ok(Html(html))
}

/// POST /settings/profile/display-name/ and its legacy alias — update the name.
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

    Ok((StatusCode::FOUND, [("Location", "/settings/profile/")]).into_response())
}

/// POST /settings/profile/language/ and its legacy alias.
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

    Ok((StatusCode::FOUND, [("Location", "/settings/profile/")]).into_response())
}

// ─── Security ───────────────────────────────────────────────────────────────

#[derive(Template)]
#[template(path = "settings/security.html")]
pub struct SecurityTemplate {
    pub urls: &'static crate::static_assets::TemplateUrls,
    pub t: &'static I18n,
    pub user_email: String,
    pub error: Option<String>,
    /// A one-shot success message from the redirect that landed here.
    pub flash: Option<&'static str>,
    pub two_fa_enabled: bool,
    pub back_to: &'static str,
    pub is_admin: bool,
    pub active_page: &'static str,
    pub nav: SettingsNav,
    pub csrf_token: Option<String>,
    pub left_panel_repos: Vec<crate::service::repo::service::LeftPanelRepo>,
    pub current_repo_id: Option<String>,
}

/// Query flags the security page understands after a redirect.
#[derive(Deserialize, Default)]
pub struct SecurityQuery {
    pub changed: Option<String>,
}

/// GET /settings/security/ — two-factor state and the password form.
pub async fn security_page(
    user: WebUser,
    State(state): State<Arc<AppState>>,
    Query(query): Query<SecurityQuery>,
) -> Result<Html<String>, AppError> {
    let ctx = crate::ui::ctx::build_page_ctx(&state, &user).await?;
    let nav = build_nav(&state, &user, SettingsNav::SECURITY).await?;
    let tpl = SecurityTemplate {
        urls: ctx.urls,
        t: ctx.t,
        user_email: ctx.user_email,
        error: None,
        flash: flash_for(query.changed.as_deref()),
        two_fa_enabled: nav.two_fa_enabled,
        back_to: "/settings/",
        is_admin: ctx.is_admin,
        active_page: SettingsNav::SECURITY,
        nav,
        csrf_token: Some(ctx.csrf_token),
        left_panel_repos: ctx.left_panel_repos,
        current_repo_id: None,
    };

    let html = tpl
        .render()
        .map_err(|e| AppError::internal(e.to_string()))?;
    Ok(Html(html))
}

/// Map a `changed` query value onto its i18n key. Anything unrecognised — a
/// hand-typed URL, a stale bookmark — renders no message rather than echoing
/// the value back into the page.
fn flash_for(changed: Option<&str>) -> Option<&'static str> {
    match changed {
        Some("password") => Some("settings.password_changed"),
        _ => None,
    }
}

/// POST /settings/password/ — change password.
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
        return render_security_error(
            &state,
            &user,
            I18n::get(user.language.as_deref())
                .tr("settings.incorrect_password")
                .to_string(),
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
        return render_security_error(&state, &user, msg).await;
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

    // Report it: the revocations above are a security event the owner should
    // be told about, not something to discover by finding other devices
    // signed out.
    Ok((
        StatusCode::FOUND,
        [("Location", "/settings/security/?changed=password")],
    )
        .into_response())
}

/// Re-render the security page with an error message.
async fn render_security_error(
    state: &Arc<AppState>,
    user: &WebUser,
    error: String,
) -> Result<Response, AppError> {
    let ctx = crate::ui::ctx::build_page_ctx(state, user).await?;
    let nav = build_nav(state, user, SettingsNav::SECURITY).await?;
    let tpl = SecurityTemplate {
        urls: ctx.urls,
        t: ctx.t,
        user_email: ctx.user_email,
        error: Some(error),
        flash: None,
        two_fa_enabled: nav.two_fa_enabled,
        back_to: "/settings/",
        is_admin: ctx.is_admin,
        active_page: SettingsNav::SECURITY,
        nav,
        csrf_token: Some(ctx.csrf_token),
        left_panel_repos: ctx.left_panel_repos,
        current_repo_id: None,
    };
    let html = tpl
        .render()
        .map_err(|e| AppError::internal(e.to_string()))?;
    Ok((StatusCode::OK, Html(html)).into_response())
}

// ─── Credentials ────────────────────────────────────────────────────────────

#[derive(Template)]
#[template(path = "settings/credentials.html")]
pub struct CredentialsTemplate {
    pub urls: &'static crate::static_assets::TemplateUrls,
    pub t: &'static I18n,
    pub user_email: String,
    pub is_admin: bool,
    pub active_page: &'static str,
    pub nav: SettingsNav,
    pub csrf_token: Option<String>,
    pub summary: crate::service::credential::CredentialSummary,
    /// Client devices with everything each one owns.
    pub devices: Vec<DeviceInfo>,
    pub browsers: Vec<BrowserInfo>,
    /// Sync tokens whose device is not in `devices`, i.e. everything except
    /// the ones already shown inside a device's expanded detail. These are
    /// historical leftovers — a normal account has none — so the section that
    /// renders them is hidden when the list is empty.
    pub loose_sync_tokens: Vec<SyncTokenInfo>,
    /// 2FA trusts whose device is not in `devices`; see `loose_sync_tokens`.
    pub loose_device_trusts: Vec<DeviceTrustInfo>,
    pub api_key_count: usize,
    pub other_browser_sessions: usize,
    pub flash: Option<&'static str>,
    pub flash_count: Option<u64>,
    pub error: Option<String>,
    pub left_panel_repos: Vec<crate::service::repo::service::LeftPanelRepo>,
    pub current_repo_id: Option<String>,
}

pub struct DeviceInfo {
    pub platform: String,
    pub platform_display: String,
    pub device_id: String,
    pub device_name: String,
    pub client_version: String,
    pub last_sign_in_ts: i64,
    pub is_desktop_client: bool,
    /// Total credentials this device owns, for the "unlink removes…" copy.
    pub credential_count: usize,
    pub sync_tokens: Vec<SyncTokenInfo>,
    pub device_trusts: Vec<DeviceTrustInfo>,
}

pub struct BrowserInfo {
    pub id: i32,
    /// i18n key naming the login this session came from.
    pub source_label: &'static str,
    /// `Chrome · macOS`, or empty when the login carried no usable agent.
    pub client: String,
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
    pub created_ts: i64,
    pub last_sync_ts: Option<i64>,
    pub expires_at: Option<i64>,
    pub is_expired: bool,
    pub expires_soon: bool,
    pub is_stale: bool,
}

pub struct DeviceTrustInfo {
    pub id: i32,
    pub device_name: Option<String>,
    pub device_id: Option<String>,
    pub created_ts: i64,
    pub expires_ts: i64,
}

/// Query flags the credentials page understands after a redirect.
#[derive(Deserialize, Default)]
pub struct CredentialsQuery {
    pub revoked: Option<String>,
    pub bulk: Option<String>,
    pub n: Option<u64>,
}

fn flash_for_revoked(revoked: Option<&str>) -> Option<&'static str> {
    match revoked {
        Some("credential") => Some("credential.revoked"),
        _ => None,
    }
}

fn flash_for_bulk(bulk: Option<&str>) -> Option<&'static str> {
    match bulk {
        Some(id) if BulkRevoke::from_id(id).is_some() => Some("credential.bulk_done"),
        _ => None,
    }
}

/// GET /settings/credentials/ — the account's credential inventory.
///
/// Everything long-lived the account holds, in one place: client sessions,
/// browser sessions, repository sync tokens, 2FA device trusts, and a pointer
/// to the API keys. The page exists because a credential the owner cannot see
/// is one they cannot revoke — which is how sync tokens, with a one-year
/// default lifetime, used to be.
pub async fn credentials_page(
    user: WebUser,
    State(state): State<Arc<AppState>>,
    Query(query): Query<CredentialsQuery>,
) -> Result<Html<String>, AppError> {
    let two_fa = state.repos.user_2fa.find_by_user_id(user.user_id).await?;
    let two_fa_enabled = two_fa.as_ref().map(|tf| tf.enabled).unwrap_or(false);
    render_credentials(&state, &user, two_fa_enabled, query, None).await
}

/// Format the inventory for the credentials page.
///
/// Shared by the GET handler and every error path, so a failed action shows the
/// same page with an alert on it rather than a bare status code.
async fn render_credentials(
    state: &Arc<AppState>,
    user: &WebUser,
    two_fa_enabled: bool,
    query: CredentialsQuery,
    error: Option<String>,
) -> Result<Html<String>, AppError> {
    let service = CredentialService::new(state.repos.clone());
    let inventory = service
        .inventory(user.user_id, Some(user.session_id))
        .await?;
    let mut summary = inventory.summary.clone();
    summary.two_fa_enabled = two_fa_enabled;

    let sync_tokens: Vec<SyncTokenInfo> =
        inventory.sync_tokens.iter().map(sync_token_info).collect();
    let device_trusts: Vec<DeviceTrustInfo> = inventory
        .device_trusts
        .iter()
        .map(device_trust_info)
        .collect();

    let devices: Vec<DeviceInfo> = inventory
        .devices
        .iter()
        .map(|device| DeviceInfo {
            platform: device.platform.clone(),
            platform_display: device.platform_display.clone(),
            device_id: device.device_id.clone(),
            device_name: device.device_name.clone(),
            client_version: device.client_version.clone(),
            last_sign_in_ts: device.created_ts,
            is_desktop_client: device.is_desktop_client,
            credential_count: 1 + device.sync_tokens.len() + device.device_trusts.len(),
            sync_tokens: device.sync_tokens.iter().map(sync_token_info).collect(),
            device_trusts: device.device_trusts.iter().map(device_trust_info).collect(),
        })
        .collect();

    // A token shown inside a device is not repeated in the flat list; the page
    // would otherwise present the same credential twice. What is left over has
    // no known device at all, which is why its section is only rendered when
    // the list is non-empty.
    let attached_token_ids: std::collections::HashSet<i32> = devices
        .iter()
        .flat_map(|d| d.sync_tokens.iter().map(|t| t.id))
        .collect();
    let attached_trust_ids: std::collections::HashSet<i32> = devices
        .iter()
        .flat_map(|d| d.device_trusts.iter().map(|t| t.id))
        .collect();
    let loose_sync_tokens: Vec<SyncTokenInfo> = sync_tokens
        .iter()
        .filter(|t| !attached_token_ids.contains(&t.id))
        .map(clone_sync_token_info)
        .collect();
    let loose_device_trusts: Vec<DeviceTrustInfo> = device_trusts
        .iter()
        .filter(|t| !attached_trust_ids.contains(&t.id))
        .map(clone_device_trust_info)
        .collect();

    let ctx = crate::ui::ctx::build_page_ctx(state, user).await?;
    let nav = SettingsNav {
        active: SettingsNav::CREDENTIALS,
        is_admin: user.is_admin,
        credential_count: inventory.browsers.len()
            + inventory.clients.len()
            + inventory.sync_tokens.len()
            + inventory.device_trusts.len(),
        api_key_count: inventory.api_key_count,
        two_fa_enabled,
    };

    let flash_count = query.n;
    let tpl = CredentialsTemplate {
        urls: ctx.urls,
        t: ctx.t,
        user_email: ctx.user_email,
        is_admin: ctx.is_admin,
        active_page: SettingsNav::CREDENTIALS,
        nav,
        csrf_token: Some(ctx.csrf_token),
        summary,
        other_browser_sessions: inventory.other_browser_sessions(),
        devices,
        browsers: inventory
            .browsers
            .iter()
            .map(|browser| BrowserInfo {
                id: browser.id,
                source_label: match browser.source {
                    crate::domain::session_source::SessionSource::WebClientLogin => {
                        "credential.source_web_client_login"
                    }
                    _ => "credential.source_web",
                },
                client: browser.client.clone(),
                signed_in_ts: browser.created_ts,
                is_current: browser.is_current,
            })
            .collect(),
        loose_sync_tokens,
        loose_device_trusts,
        api_key_count: inventory.api_key_count,
        flash: flash_for_revoked(query.revoked.as_deref())
            .or_else(|| flash_for_bulk(query.bulk.as_deref())),
        flash_count,
        error,
        left_panel_repos: ctx.left_panel_repos,
        current_repo_id: None,
    };

    let html = tpl
        .render()
        .map_err(|e| AppError::internal(e.to_string()))?;
    Ok(Html(html))
}

fn sync_token_info(token: &crate::service::credential::SyncTokenView) -> SyncTokenInfo {
    let now = chrono::Utc::now().timestamp();
    SyncTokenInfo {
        id: token.id,
        repo_name: token.repo_name.clone(),
        device_name: token.device_name.clone(),
        peer_ip: token.peer_ip.clone(),
        client_version: token.client_version.clone(),
        created_ts: token.created_ts,
        last_sync_ts: token.last_sync_ts,
        expires_at: token.expires_at,
        is_expired: token.is_expired(now),
        expires_soon: token.expires_soon(now),
        is_stale: token.is_stale(now),
    }
}

fn device_trust_info(trust: &crate::service::credential::DeviceTrustView) -> DeviceTrustInfo {
    DeviceTrustInfo {
        id: trust.id,
        device_name: trust.device_name.clone(),
        device_id: trust.device_id.clone(),
        created_ts: trust.created_ts,
        expires_ts: trust.expires_at,
    }
}

fn clone_sync_token_info(token: &SyncTokenInfo) -> SyncTokenInfo {
    SyncTokenInfo {
        id: token.id,
        repo_name: token.repo_name.clone(),
        device_name: token.device_name.clone(),
        peer_ip: token.peer_ip.clone(),
        client_version: token.client_version.clone(),
        created_ts: token.created_ts,
        last_sync_ts: token.last_sync_ts,
        expires_at: token.expires_at,
        is_expired: token.is_expired,
        expires_soon: token.expires_soon,
        is_stale: token.is_stale,
    }
}

fn clone_device_trust_info(trust: &DeviceTrustInfo) -> DeviceTrustInfo {
    DeviceTrustInfo {
        id: trust.id,
        device_name: trust.device_name.clone(),
        device_id: trust.device_id.clone(),
        created_ts: trust.created_ts,
        expires_ts: trust.expires_ts,
    }
}

/// POST /settings/credentials/revoke/ — revoke one credential by kind and id.
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
    if let Err(err) = service.revoke(user.user_id, kind, form.id).await {
        return render_credentials_error(&state, &user, err).await;
    }

    let location = if signing_out_self {
        "/accounts/login/".to_string()
    } else {
        "/settings/credentials/?revoked=credential".to_string()
    };
    Ok((StatusCode::FOUND, [("Location", location)]).into_response())
}

/// POST /settings/credentials/bulk/ — sign out other browsers or drop tokens.
///
/// `keep_session_id` comes from the authenticated session, never from the
/// form: the one thing this action must not do is sign the actor out.
pub async fn revoke_bulk(
    user: WebUser,
    State(state): State<Arc<AppState>>,
    Form(form): Form<BulkRevokeForm>,
) -> Result<impl IntoResponse, AppError> {
    crate::service::auth::csrf::check_form_csrf(
        &state,
        &user.session_token,
        form.csrf_token.as_deref(),
    )?;

    let what = BulkRevoke::from_id(&form.action)
        .ok_or_else(|| AppError::BadRequest("unknown bulk action".into()))?;

    let service = CredentialService::new(state.repos.clone());
    let removed = match service
        .revoke_bulk(user.user_id, what, Some(user.session_id))
        .await
    {
        Ok(removed) => removed,
        Err(err) => return render_credentials_error(&state, &user, err).await,
    };

    let location = if removed == 0 {
        "/settings/credentials/?bulk=nothing".to_string()
    } else {
        format!("/settings/credentials/?bulk={}&n={}", what.id(), removed)
    };
    Ok((StatusCode::FOUND, [("Location", location)]).into_response())
}

/// Re-render the credentials page with an error message.
///
/// The status is kept honest: a caller that asked to revoke something it does
/// not own gets a 404, not a 200 with an error printed on it.
async fn render_credentials_error(
    state: &Arc<AppState>,
    user: &WebUser,
    err: AppError,
) -> Result<Response, AppError> {
    let status = match err {
        AppError::NotFound(_) => StatusCode::NOT_FOUND,
        AppError::Forbidden => StatusCode::FORBIDDEN,
        _ => StatusCode::BAD_REQUEST,
    };
    let two_fa = state.repos.user_2fa.find_by_user_id(user.user_id).await?;
    let two_fa_enabled = two_fa.as_ref().map(|tf| tf.enabled).unwrap_or(false);
    let Html(html) = render_credentials(
        state,
        user,
        two_fa_enabled,
        CredentialsQuery::default(),
        Some(err.to_string()),
    )
    .await?;
    Ok((status, Html(html)).into_response())
}

/// POST /settings/credentials/unlink/ (and the legacy
/// `/settings/devices/`) — remove a device's tokens (API, S2FA, sync).
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

    Ok((
        StatusCode::FOUND,
        [("Location", "/settings/credentials/?revoked=credential")],
    )
        .into_response())
}

/// GET /settings/devices/ — the page this one replaced.
pub async fn redirect_credentials() -> impl IntoResponse {
    (StatusCode::FOUND, [("Location", "/settings/credentials/")])
}

// ─── Avatar upload (web UI) ──────────────────────────────────────────────────

/// POST /settings/profile/avatar/ and its legacy alias — upload a new avatar.
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
                        return render_profile_error(
                            &state,
                            &user,
                            I18n::get(user.language.as_deref())
                                .tr("settings.upload_avatar_failed")
                                .to_string(),
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
        return render_profile_error(
            &state,
            &user,
            I18n::get(user.language.as_deref())
                .tr("common.invalid_csrf")
                .to_string(),
        )
        .await;
    }

    let (file_name, data) =
        avatar_field.ok_or_else(|| AppError::BadRequest("no avatar file provided".into()))?;

    // Delegate to the shared AvatarService which handles validation (size/ext),
    // persistence, thumbnail generation (with square crop + EXIF), and DB upsert.
    let svc = state.avatar_service();
    match svc.upload_avatar(&user.email, file_name, data).await {
        Ok(_url) => Ok((StatusCode::FOUND, [("Location", "/settings/profile/")]).into_response()),
        Err(e) => {
            let msg = match &e {
                AppError::BadRequest(m) => m.clone(),
                _ => I18n::get(user.language.as_deref())
                    .tr("settings.upload_avatar_failed")
                    .to_string(),
            };
            render_profile_error(&state, &user, msg).await
        }
    }
}

/// Re-render the profile page with an error message.
async fn render_profile_error(
    state: &Arc<AppState>,
    user: &WebUser,
    error: String,
) -> Result<Response, AppError> {
    let user_record = state
        .repos
        .user
        .find_by_id(user.user_id)
        .await?
        .ok_or_else(|| AppError::NotFound("User not found".to_string()))?;
    let ctx = crate::ui::ctx::build_page_ctx(state, user).await?;
    let nav = build_nav(state, user, SettingsNav::PROFILE).await?;
    let tpl = ProfileTemplate {
        urls: ctx.urls,
        t: ctx.t,
        user_language: ctx.t.lang,
        user_email: ctx.user_email,
        user_display_name: user_record.nickname(),
        error: Some(error),
        success: None,
        is_admin: ctx.is_admin,
        active_page: SettingsNav::PROFILE,
        nav,
        csrf_token: Some(ctx.csrf_token),
        left_panel_repos: ctx.left_panel_repos,
        current_repo_id: None,
    };
    let html = tpl
        .render()
        .map_err(|e| AppError::internal(e.to_string()))?;
    Ok((StatusCode::OK, Html(html)).into_response())
}

// ─── Forms ──────────────────────────────────────────────────────────────────

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
pub struct BulkRevokeForm {
    /// `sign_out_others`; see [`BulkRevoke`].
    pub action: String,
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
