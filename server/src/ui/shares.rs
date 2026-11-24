use askama::Template;
use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::{Html, IntoResponse},
};
use serde::Deserialize;
use std::sync::Arc;

use crate::AppState;
use crate::i18n::I18n;
use base::error::AppError;

use super::auth_extractor::WebUser;
use super::shares_data::{LinkScope, load_links};

pub type ShareLinkInfo = super::shares_data::ShareLinkRow;
pub type UploadLinkInfo = super::shares_data::UploadLinkRow;

#[derive(Template)]
#[template(path = "shares/list.html")]
pub struct SharesTemplate {
    pub urls: &'static crate::static_assets::TemplateUrls,
    pub t: &'static I18n,
    pub user_email: String,
    pub is_admin: bool,
    pub csrf_token: String,
    pub share_links: Vec<ShareLinkInfo>,
    pub upload_links: Vec<UploadLinkInfo>,
    pub active_page: &'static str,
    pub active_tab: String,
    pub left_panel_repos: Vec<crate::service::repo::service::LeftPanelRepo>,
    pub current_repo_id: Option<String>,
    /// Either direction is open: the left panel's Shares link and the page itself
    /// exist only while that holds.
    pub sharing_enabled: bool,
    pub share_download_enabled: bool,
    pub share_upload_enabled: bool,
}

#[derive(Deserialize)]
pub struct CreateShareForm {
    pub repo_id: String,
    pub path: String,
    pub csrf_token: Option<String>,
}

#[derive(Deserialize)]
pub struct SharesQuery {
    pub tab: Option<String>,
}

/// GET /shares/ — list all share links.
pub async fn list_shares(
    user: WebUser,
    State(state): State<Arc<AppState>>,
    Query(query): Query<SharesQuery>,
) -> Result<Html<String>, AppError> {
    let share_download_enabled = state.config().server.share_download_enabled;
    let share_upload_enabled = state.config().server.share_upload_enabled;
    // Both halves switched off closes the page itself: there is nothing to manage
    // and the navigation no longer links here.
    if !share_download_enabled && !share_upload_enabled {
        return Err(AppError::NotFound("sharing is turned off".into()));
    }

    let links = load_links(&state, LinkScope::Own(user.user_id)).await?;
    let share_links = links.share_links;
    let upload_links = links.upload_links;

    // A `?tab=` naming a switched-off direction falls back to the one that is open,
    // so a bookmark left behind by a stricter setting still lands somewhere useful.
    let active_tab = match query.tab.as_deref() {
        Some("upload-links") if share_upload_enabled => "upload-links",
        Some("share-links") if share_download_enabled => "share-links",
        _ if share_download_enabled => "share-links",
        _ => "upload-links",
    }
    .to_string();

    let ctx = crate::ui::ctx::build_page_ctx(&state, &user).await?;
    let tpl = SharesTemplate {
        urls: ctx.urls,
        t: ctx.t,
        user_email: ctx.user_email,
        is_admin: ctx.is_admin,
        csrf_token: ctx.csrf_token,
        share_links,
        upload_links,
        active_page: "shares",
        active_tab,
        left_panel_repos: ctx.left_panel_repos,
        current_repo_id: None,
        sharing_enabled: ctx.sharing_enabled,
        share_download_enabled,
        share_upload_enabled,
    };

    let html = tpl
        .render()
        .map_err(|e| AppError::internal(e.to_string()))?;
    Ok(Html(html))
}

/// POST /shares/create/ — create a share link; delegates to the v2/v2.1 service
/// so the encrypted-library, permission and path checks cannot diverge.
pub async fn create_share(
    user: WebUser,
    State(state): State<Arc<AppState>>,
    axum::Form(form): axum::Form<CreateShareForm>,
) -> Result<impl IntoResponse, AppError> {
    crate::middleware::ensure_share_download_enabled(&state)?;
    crate::service::auth::csrf::check_form_csrf(
        &state,
        &user.session_token,
        form.csrf_token.as_deref(),
    )?;

    crate::service::sharing::share::create_share_link_impl(
        &state.repos,
        &state.config(),
        &form.repo_id,
        &form.path,
        None,
        None,
        None,
        user.user_id,
        &crate::domain::permission::RepoScope::all(),
    )
    .await?;

    Ok((StatusCode::FOUND, [("Location", "/shares/")]).into_response())
}

/// POST /share/{token}/delete — delete a share link.
pub async fn delete_share(
    user: WebUser,
    State(state): State<Arc<AppState>>,
    Path(token): Path<String>,
    axum::Form(form): axum::Form<std::collections::HashMap<String, String>>,
) -> Result<impl IntoResponse, AppError> {
    crate::service::auth::csrf::check_form_csrf(
        &state,
        &user.session_token,
        form.get("csrf_token").map(|s| s.as_str()),
    )?;

    let deleted =
        super::shares_data::delete_share(&state.repos, &token, user.user_id, false).await?;
    if !deleted {
        return Err(AppError::NotFound("Share link not found".to_string()));
    }

    let redirect = match form.get("tab").map(|s| s.as_str()) {
        Some("upload-links") => "/shares/?tab=upload-links",
        _ => "/shares/",
    };
    Ok((StatusCode::FOUND, [("Location", redirect)]).into_response())
}

/// POST /shares/upload/{token}/delete/ — delete an upload link.
pub async fn delete_upload(
    user: WebUser,
    State(state): State<Arc<AppState>>,
    Path(token): Path<String>,
    axum::Form(form): axum::Form<std::collections::HashMap<String, String>>,
) -> Result<impl IntoResponse, AppError> {
    crate::service::auth::csrf::check_form_csrf(
        &state,
        &user.session_token,
        form.get("csrf_token").map(|s| s.as_str()),
    )?;

    let deleted = super::shares_data::delete_upload(
        &state.repos,
        &state.token_manager,
        &token,
        user.user_id,
        false,
    )
    .await?;
    if !deleted {
        return Err(AppError::NotFound("Upload link not found".to_string()));
    }

    let redirect = match form.get("tab").map(|s| s.as_str()) {
        Some("upload-links") => "/shares/?tab=upload-links",
        _ => "/shares/",
    };
    Ok((StatusCode::FOUND, [("Location", redirect)]).into_response())
}
