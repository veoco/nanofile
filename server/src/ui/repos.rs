//! Web UI repo handlers — list repos.
use askama::Template;
use axum::{extract::State, response::Html};
use std::sync::Arc;

use crate::AppState;
use crate::i18n::I18n;
use crate::ui::files::format_size;
use base::error::AppError;

use super::auth_extractor::WebUser;

/// One option in the administrator's share dialog user picker.
pub struct ShareUserOption {
    pub email: String,
    pub name: String,
}

#[derive(Template)]
#[template(path = "repos/list.html")]
pub struct RepoListTemplate {
    pub urls: &'static crate::static_assets::TemplateUrls,
    pub t: &'static I18n,
    pub user_email: String,
    pub is_admin: bool,
    pub repos: Vec<RepoInfo>,
    pub share_users: Vec<ShareUserOption>,
    pub active_page: &'static str,
    pub csrf_token: String,
    pub left_panel_repos: Vec<crate::service::repo::service::LeftPanelRepo>,
    pub sharing_enabled: bool,
    pub current_repo_id: Option<String>,
    /// Base URL (from site_url) used to build the WebDAV endpoint address.
    pub webdav_base_url: String,
}

pub struct RepoInfo {
    pub id: String,
    pub name: String,
    pub description: String,
    pub size: i64,
    pub size_display: String,
    pub mtime: i64,
    pub encrypted: bool,
    pub history_limit: i32,
    pub history_ttl_days: i32,
    /// Real owner of the library (may be another account for shared/public).
    pub owner_email: String,
    pub owner_name: String,
    /// The caller owns the library, so the row can offer edit/delete/share.
    pub is_owner: bool,
    /// `private` / `shared` / `public`; drives the badge and the share dialog.
    pub visibility: String,
    /// `rw` or `r` for this caller.
    pub permission: String,
}

/// GET /libraries/ — list user's repos.
pub async fn list_repos(
    user: WebUser,
    State(state): State<Arc<AppState>>,
) -> Result<Html<String>, AppError> {
    let rows = crate::service::repo::service::RepoService::load_repo_access(
        &state.repos,
        user.user_id,
        &user.email,
    )
    .await?;

    let mut repos = Vec::with_capacity(rows.len());
    for row in rows {
        let is_owner = row.repo.owner_id == user.user_id;
        let r = row.repo;
        repos.push(RepoInfo {
            id: r.id,
            name: r.name,
            description: r.description,
            size: r.size,
            size_display: format_size(r.size),
            mtime: r.updated_at,
            encrypted: r.encrypted != 0,
            history_limit: r.history_limit,
            history_ttl_days: r.history_ttl_days,
            owner_email: row.owner_email,
            owner_name: row.owner_name,
            is_owner,
            visibility: row.visibility,
            permission: row.permission,
        });
    }

    let ctx = crate::ui::ctx::build_page_ctx(&state, &user).await?;

    let mut share_users = Vec::new();
    if user.is_admin {
        // The instance is small; a server-rendered picker keeps the dialog free
        // of a second round-trip and of an exposed public user-search endpoint.
        let mut users: Vec<_> = state
            .repos
            .user
            .find_all()
            .await?
            .into_iter()
            .filter(|u| u.is_active)
            .collect();
        users.sort_by_key(|u| u.email.to_lowercase());
        share_users = users
            .into_iter()
            .map(|u| ShareUserOption {
                name: u.nickname(),
                email: u.email,
            })
            .collect();
    }

    // Render in the list's default order (name, A→Z) so the first paint is
    // already final — browser-side sorting reordered rows and read as a flicker.
    repos.sort_by_key(|a| a.name.to_lowercase());

    let tpl = RepoListTemplate {
        urls: ctx.urls,
        t: ctx.t,
        user_email: ctx.user_email,
        is_admin: ctx.is_admin,
        repos,
        share_users,
        active_page: "repos",
        csrf_token: ctx.csrf_token,
        left_panel_repos: ctx.left_panel_repos,
        sharing_enabled: ctx.sharing_enabled,
        current_repo_id: None,
        webdav_base_url: state
            .config()
            .server
            .site_url
            .trim_end_matches('/')
            .to_string(),
    };

    let html = tpl
        .render()
        .map_err(|e| AppError::internal(e.to_string()))?;
    Ok(Html(html))
}
