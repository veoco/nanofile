/// Web UI repo handlers — list repos.
use askama::Template;
use axum::{extract::State, response::Html};
use std::sync::Arc;

use crate::AppState;
use crate::i18n::I18n;
use crate::ui::files::format_size;
use base::error::AppError;

use super::auth_extractor::WebUser;

#[derive(Template)]
#[template(path = "repos/list.html")]
pub struct RepoListTemplate {
    pub urls: &'static crate::static_assets::TemplateUrls,
    pub t: &'static I18n,
    pub user_email: String,
    pub is_admin: bool,
    pub repos: Vec<RepoInfo>,
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
        });
    }

    let ctx = crate::ui::ctx::build_page_ctx(&state, &user).await?;

    // Render in the list's default order (name, A→Z) so the first paint is
    // already final — browser-side sorting reordered rows and read as a flicker.
    repos.sort_by_key(|a| a.name.to_lowercase());

    let tpl = RepoListTemplate {
        urls: ctx.urls,
        t: ctx.t,
        user_email: ctx.user_email,
        is_admin: ctx.is_admin,
        repos,
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
