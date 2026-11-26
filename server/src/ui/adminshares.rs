//! Admin Web UI — share management (view/delete all share and upload links).
use askama::Template;
use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::{Html, IntoResponse, Response},
};
use serde::Deserialize;
use std::sync::Arc;

use crate::AppState;
use crate::i18n::I18n;
use base::error::AppError;

use super::auth_extractor::WebUser;
use super::shares_data::{LinkScope, load_links};

pub type AdminShareLinkInfo = super::shares_data::ShareLinkRow;
pub type AdminUploadLinkInfo = super::shares_data::UploadLinkRow;

#[derive(Template)]
#[template(path = "adminshares/list.html")]
pub struct AdminSharesTemplate {
    pub urls: &'static crate::static_assets::TemplateUrls,
    pub t: &'static I18n,
    pub user_email: String,
    pub is_admin: bool,
    pub csrf_token: Option<String>,
    pub share_links: Vec<AdminShareLinkInfo>,
    pub upload_links: Vec<AdminUploadLinkInfo>,
    pub active_page: &'static str,
    pub active_tab: String,
    pub error: Option<String>,
    pub success: Option<String>,
    pub left_panel_repos: Vec<crate::service::repo::service::LeftPanelRepo>,
    pub sharing_enabled: bool,
    pub current_repo_id: Option<String>,
    pub share_download_enabled: bool,
    pub share_upload_enabled: bool,
}

#[derive(Deserialize)]
pub struct AdminSharesQuery {
    pub tab: Option<String>,
    pub action: Option<String>,
}

/// The one tab name the page accepts; anything else means the first tab.
fn normalize_tab(tab: Option<&str>) -> String {
    match tab {
        Some("upload-links") => "upload-links".to_string(),
        _ => "share-links".to_string(),
    }
}

/// GET /sysadmin/shares/ — list all share and upload links (admin only).
pub async fn list_all_shares(
    user: WebUser,
    State(state): State<Arc<AppState>>,
    Query(query): Query<AdminSharesQuery>,
) -> Response {
    if !user.is_admin {
        return axum::response::Redirect::to("/libraries/").into_response();
    }

    let success = match query.action.as_deref() {
        Some("deleted") => Some(
            I18n::get(user.language.as_deref())
                .tr("admin.link_deleted")
                .to_string(),
        ),
        _ => None,
    };

    let active_tab = normalize_tab(query.tab.as_deref());
    match render_page(&state, &user, active_tab, None, success).await {
        Ok(resp) => resp,
        Err(e) => e.into_response(),
    }
}

async fn render_page(
    state: &Arc<AppState>,
    user: &WebUser,
    active_tab: String,
    error: Option<String>,
    success: Option<String>,
) -> Result<Response, AppError> {
    let links = load_links(state, LinkScope::All).await?;
    let ctx = crate::ui::ctx::build_page_ctx(state, user).await?;

    let tpl = AdminSharesTemplate {
        urls: ctx.urls,
        t: ctx.t,
        user_email: ctx.user_email,
        is_admin: ctx.is_admin,
        csrf_token: Some(ctx.csrf_token),
        share_links: links.share_links,
        upload_links: links.upload_links,
        active_page: "adminshares",
        active_tab,
        error,
        success,
        left_panel_repos: ctx.left_panel_repos,
        sharing_enabled: ctx.sharing_enabled,
        current_repo_id: None,
        share_download_enabled: state.config().server.share_download_enabled,
        share_upload_enabled: state.config().server.share_upload_enabled,
    };

    let html = tpl
        .render()
        .map_err(|e| AppError::internal(e.to_string()))?;
    Ok(Html(html).into_response())
}

/// POST /sysadmin/shares/share/{token}/delete/ — delete any share link (admin).
pub async fn delete_share(
    user: WebUser,
    State(state): State<Arc<AppState>>,
    Path(token): Path<String>,
    axum::Form(form): axum::Form<std::collections::HashMap<String, String>>,
) -> Result<impl IntoResponse, AppError> {
    if !user.is_admin {
        return Err(AppError::Forbidden);
    }
    crate::middleware::csrf::check_form_csrf(
        &state,
        &user.session_token,
        form.get("csrf_token").map(String::as_str),
    )?;

    // Admin delete: no creator check. A stale page or double submit reports the
    // link as already gone instead of erroring.
    let tab = normalize_tab(form.get("tab").map(String::as_str));
    let deleted = match crate::service::sharing::share::delete_share_link_as_admin(
        &state.repos,
        &token,
    )
    .await
    {
        Ok(deleted) => deleted,
        Err(e) => {
            let msg = crate::ui::banner::action_error(I18n::get(user.language.as_deref()), &e);
            return render_page(&state, &user, tab, Some(msg), None).await;
        }
    };
    if !deleted {
        let msg = I18n::get(user.language.as_deref())
            .tr("admin.link_not_found")
            .to_string();
        return render_page(&state, &user, tab, Some(msg), None).await;
    }

    Ok((StatusCode::FOUND, [("Location", deleted_location(&tab))]).into_response())
}

/// POST /sysadmin/shares/upload/{token}/delete/ — delete any upload link (admin).
pub async fn delete_upload(
    user: WebUser,
    State(state): State<Arc<AppState>>,
    Path(token): Path<String>,
    axum::Form(form): axum::Form<std::collections::HashMap<String, String>>,
) -> Result<impl IntoResponse, AppError> {
    if !user.is_admin {
        return Err(AppError::Forbidden);
    }
    crate::middleware::csrf::check_form_csrf(
        &state,
        &user.session_token,
        form.get("csrf_token").map(String::as_str),
    )?;

    let tab = normalize_tab(form.get("tab").map(String::as_str));
    let deleted = match crate::service::sharing::link::delete_upload_link_as_admin(
        &state.repos,
        &token,
        Some(&state.token_manager),
    )
    .await
    {
        Ok(deleted) => deleted,
        Err(e) => {
            let msg = crate::ui::banner::action_error(I18n::get(user.language.as_deref()), &e);
            return render_page(&state, &user, tab, Some(msg), None).await;
        }
    };
    if !deleted {
        let msg = I18n::get(user.language.as_deref())
            .tr("admin.link_not_found")
            .to_string();
        return render_page(&state, &user, tab, Some(msg), None).await;
    }

    Ok((StatusCode::FOUND, [("Location", deleted_location(&tab))]).into_response())
}

fn deleted_location(tab: &str) -> String {
    if tab == "upload-links" {
        "/sysadmin/shares/?tab=upload-links&action=deleted".to_string()
    } else {
        "/sysadmin/shares/?action=deleted".to_string()
    }
}
