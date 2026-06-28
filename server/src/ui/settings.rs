/// Web UI settings handlers — account info, password change, devices.
use askama::Template;
use axum::{
    Form,
    extract::{Multipart, State},
    http::StatusCode,
    response::{Html, IntoResponse, Response},
};
use sea_orm::{ActiveModelTrait, ColumnTrait, EntityTrait, QueryFilter, QueryOrder, Set};
use serde::Deserialize;
use std::sync::Arc;

use crate::AppState;
use crate::auth::password::{hash_password, verify_password};
use crate::entity::{api_token, s2fa_token, sync_token, user, user_2fa};
use crate::error::AppError;

use super::auth_extractor::WebUser;

#[derive(Template)]
#[template(path = "settings/index.html")]
pub struct SettingsTemplate {
    pub urls: &'static crate::static_assets::TemplateUrls,
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
    pub left_panel_repos: Vec<crate::repo::LeftPanelRepo>,
    pub current_repo_id: Option<String>,
}

#[derive(Template)]
#[template(path = "settings/devices.html")]
pub struct DevicesTemplate {
    pub urls: &'static crate::static_assets::TemplateUrls,
    pub user_email: String,
    pub is_admin: bool,
    pub active_page: &'static str,
    pub devices: Vec<DeviceInfo>,
    pub error: Option<String>,
    pub success: Option<String>,
    pub csrf_token: Option<String>,
    pub left_panel_repos: Vec<crate::repo::LeftPanelRepo>,
    pub current_repo_id: Option<String>,
}

pub struct DeviceInfo {
    pub platform: String,
    pub device_id: String,
    pub device_name: String,
    pub client_version: String,
    pub last_accessed: i64,
    pub is_desktop_client: bool,
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
pub struct DisplayNameForm {
    pub display_name: String,
    pub csrf_token: Option<String>,
}

/// GET /profile/ — account settings page.
pub async fn settings_page(
    user: WebUser,
    State(state): State<Arc<AppState>>,
) -> Result<Html<String>, AppError> {
    let db = state.db.as_ref();

    let user_record = user::Entity::find_by_id(user.user_id)
        .one(db)
        .await?
        .ok_or_else(|| AppError::NotFound("User not found".to_string()))?;

    let two_fa = user_2fa::Entity::find_by_id(user.user_id).one(db).await?;
    let two_fa_enabled = two_fa.as_ref().map(|tf| tf.enabled).unwrap_or(false);

    let csrf_token = Some(crate::auth::csrf::generate_csrf_token(
        &state.csrf_secret,
        &user.session_token,
    ));

    let left_panel_repos =
        crate::repo::load_left_panel_repos(state.db.as_ref(), user.user_id).await?;
    let tpl = SettingsTemplate {
        urls: crate::static_assets::template_urls(),
        user_email: user.email,
        user_display_name: user_record.nickname(),
        error: None,
        success: None,
        active_page: "settings",
        two_fa_enabled,
        csrf_token,
        is_admin: user.is_admin,
        left_panel_repos,
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
    // CSRF check
    if let Some(ref token) = form.csrf_token {
        let expected =
            crate::auth::csrf::generate_csrf_token(&state.csrf_secret, &user.session_token);
        if *token != expected {
            return Err(AppError::BadRequest("Invalid CSRF token.".to_string()));
        }
    }

    let db = state.db.as_ref();

    let user_record = user::Entity::find_by_id(user.user_id)
        .one(db)
        .await
        .map_err(|e| AppError::internal(format!("db error: {e}")))?
        .ok_or_else(|| AppError::NotFound("User not found".to_string()))?;

    if !verify_password(
        &form.old_password,
        &user_record.password_hash,
        state.config.auth.password_hash_iterations,
    ) {
        let csrf_token = Some(crate::auth::csrf::generate_csrf_token(
            &state.csrf_secret,
            &user.session_token,
        ));
        let left_panel_repos =
            crate::repo::load_left_panel_repos(state.db.as_ref(), user.user_id).await?;
        let tpl = SettingsTemplate {
            urls: crate::static_assets::template_urls(),
            user_email: user.email.clone(),
            user_display_name: user.email.split('@').next().unwrap_or("").to_string(),
            error: Some("Incorrect current password.".to_string()),
            success: None,
            active_page: "settings",
            two_fa_enabled: false,
            csrf_token,
            is_admin: user.is_admin,
            left_panel_repos,
            current_repo_id: None,
        };
        let html = tpl
            .render()
            .map_err(|e| AppError::internal(e.to_string()))?;
        return Ok((StatusCode::OK, Html(html)).into_response());
    }

    let new_hash = hash_password(
        &form.new_password,
        state.config.auth.password_hash_iterations,
    );
    let mut active: user::ActiveModel = user_record.into();
    active.password_hash = Set(new_hash);
    active
        .update(db)
        .await
        .map_err(|e| AppError::internal(format!("update failed: {e}")))?;

    Ok((StatusCode::FOUND, [("Location", "/settings/")]).into_response())
}

/// POST /profile/display-name — update the user's display name.
pub async fn update_display_name(
    user: WebUser,
    State(state): State<Arc<AppState>>,
    Form(form): Form<DisplayNameForm>,
) -> Result<impl IntoResponse, AppError> {
    // CSRF check
    if let Some(ref token) = form.csrf_token {
        let expected =
            crate::auth::csrf::generate_csrf_token(&state.csrf_secret, &user.session_token);
        if *token != expected {
            return Err(AppError::BadRequest("Invalid CSRF token.".to_string()));
        }
    }

    let db = state.db.as_ref();

    let user_record = user::Entity::find_by_id(user.user_id)
        .one(db)
        .await?
        .ok_or_else(|| AppError::NotFound("User not found".to_string()))?;

    let mut active: user::ActiveModel = user_record.into();
    active.display_name = sea_orm::Set(if form.display_name.trim().is_empty() {
        None
    } else {
        Some(form.display_name.trim().to_string())
    });
    active.update(db).await?;

    Ok((StatusCode::FOUND, [("Location", "/settings/")]).into_response())
}

/// GET /profile/devices/ — device management page.
pub async fn devices_page(
    user: WebUser,
    State(state): State<Arc<AppState>>,
) -> Result<Html<String>, AppError> {
    let db = state.db.as_ref();

    let tokens = api_token::Entity::find()
        .filter(api_token::Column::UserId.eq(user.user_id))
        .filter(api_token::Column::Platform.is_not_null())
        .order_by_desc(api_token::Column::CreatedAt)
        .all(db)
        .await?;

    let mut seen = std::collections::HashSet::new();
    let mut devices = Vec::new();

    for token in tokens {
        let dev_key = (token.platform.clone(), token.device_id.clone());
        if seen.insert(dev_key) {
            let is_desktop = matches!(
                token.platform.as_deref(),
                Some("windows") | Some("linux") | Some("mac")
            );
            devices.push(DeviceInfo {
                platform: token.platform.unwrap_or_default(),
                device_id: token.device_id.unwrap_or_default(),
                device_name: token.device_name.unwrap_or_default(),
                client_version: token.client_version.unwrap_or_default(),
                last_accessed: token.created_at,
                is_desktop_client: is_desktop,
            });
        }
    }

    let csrf_token = Some(crate::auth::csrf::generate_csrf_token(
        &state.csrf_secret,
        &user.session_token,
    ));
    let left_panel_repos =
        crate::repo::load_left_panel_repos(state.db.as_ref(), user.user_id).await?;

    let tpl = DevicesTemplate {
        urls: crate::static_assets::template_urls(),
        user_email: user.email,
        is_admin: user.is_admin,
        active_page: "settings",
        devices,
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

/// POST /profile/devices/ — remove a device's tokens (API, S2FA, sync).
pub async fn unlink_device(
    user: WebUser,
    State(state): State<Arc<AppState>>,
    Form(form): Form<UnlinkDeviceForm>,
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

    // 1. Delete API tokens for this device (identified by platform + device_id).
    api_token::Entity::delete_many()
        .filter(api_token::Column::UserId.eq(user.user_id))
        .filter(api_token::Column::Platform.eq(&form.platform))
        .filter(api_token::Column::DeviceId.eq(&form.device_id))
        .exec(db)
        .await?;

    // 2. Delete S2FA device trust tokens (identified by device_id).
    s2fa_token::Entity::delete_many()
        .filter(s2fa_token::Column::UserId.eq(user.user_id))
        .filter(s2fa_token::Column::DeviceId.eq(&form.device_id))
        .exec(db)
        .await?;

    // 3. Delete sync tokens linked to this device via peer_id (= client_id).
    sync_token::Entity::delete_many()
        .filter(sync_token::Column::UserId.eq(user.user_id))
        .filter(sync_token::Column::PeerId.eq(&form.device_id))
        .exec(db)
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
    let db = state.db.as_ref();

    // Extract the avatar file from the multipart stream.
    let mut avatar_field: Option<(String, Vec<u8>)> = None;
    let mut csrf_token: Option<String> = None;

    while let Some(field) = multipart
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
                let data = field.bytes().await.unwrap_or_default().to_vec();
                avatar_field = Some((file_name, data));
            }
            _ => {}
        }
    }

    // CSRF check
    let expected_csrf =
        crate::auth::csrf::generate_csrf_token(&state.csrf_secret, &user.session_token);
    if csrf_token.as_deref() != Some(&expected_csrf) {
        return render_settings_error(&state, &user, Some("Invalid CSRF token.".to_string())).await;
    }

    let (file_name, data) =
        avatar_field.ok_or_else(|| AppError::BadRequest("no avatar file provided".into()))?;

    // Delegate to the shared AvatarService which handles validation (size/ext),
    // persistence, thumbnail generation (with square crop + EXIF), and DB upsert.
    let svc = crate::user::service::AvatarService::new(db, &state.repos);
    match svc.upload_avatar(&user.email, file_name, data).await {
        Ok(_url) => Ok((StatusCode::FOUND, [("Location", "/settings/")]).into_response()),
        Err(e) => {
            let msg = match &e {
                AppError::BadRequest(m) => m.clone(),
                _ => "Failed to upload avatar.".to_string(),
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
    let csrf_new = Some(crate::auth::csrf::generate_csrf_token(
        &state.csrf_secret,
        &user.session_token,
    ));
    let left_panel_repos =
        crate::repo::load_left_panel_repos(state.db.as_ref(), user.user_id).await?;
    let tpl = SettingsTemplate {
        urls: crate::static_assets::template_urls(),
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
