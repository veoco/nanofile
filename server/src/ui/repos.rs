/// Web UI repo handlers — list repos.
use askama::Template;
use axum::{extract::State, response::Html};
use std::sync::Arc;

use crate::AppState;
use crate::i18n::I18n;
use crate::ui::files::format_size;
use base::error::AppError;

use super::auth_extractor::WebUser;

// ─── Templates ───────────────────────────────────────────────────────────────

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
    pub current_repo_id: Option<String>,
    /// Base URL (from site_url) used to build the WebDAV endpoint address.
    pub webdav_base_url: String,
}

// ─── Data types ──────────────────────────────────────────────────────────────

pub struct RepoInfo {
    pub id: String,
    pub name: String,
    pub description: String,
    pub size: i64,
    pub size_display: String,
    pub mtime: i64,
    /// Whether the library is encrypted.
    pub encrypted: bool,
    pub history_limit: i32,
    pub history_ttl_days: i32,
}

// ─── Handlers ────────────────────────────────────────────────────────────────

/// GET /libraries/ — list user's repos.
pub async fn list_repos(
    user: WebUser,
    State(state): State<Arc<AppState>>,
) -> Result<Html<String>, AppError> {
    // Find repos where user is a member
    let memberships = state.repos.member.find_by_user_id(user.user_id).await?;
    let t = I18n::get(user.language.as_deref());

    let mut repos = Vec::new();
    for membership in memberships {
        if let Some(r) = state.repos.repo.find_by_id(&membership.repo_id).await? {
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
    }

    let csrf_token =
        crate::service::auth::csrf::generate_csrf_token(&state.csrf_secret, &user.session_token);

    // The rail keeps the membership order every other page uses; only the page
    // list is reordered.
    let left_panel_repos: Vec<crate::service::repo::service::LeftPanelRepo> = repos
        .iter()
        .map(|r| crate::service::repo::service::LeftPanelRepo {
            id: r.id.clone(),
            name: r.name.clone(),
            size_display: r.size_display.clone(),
        })
        .collect();

    // Render in the list's default order (name, A→Z) so the first paint is
    // already final: sorting only in the browser reordered every row the moment
    // the bundle ran, which read as a flicker. Lowercased, this is the same
    // comparison the file list's name sort uses and the one repos.js repeats.
    repos.sort_by_key(|a| a.name.to_lowercase());

    let tpl = RepoListTemplate {
        urls: crate::static_assets::template_urls(),
        t,
        user_email: user.email,
        is_admin: user.is_admin,
        repos,
        active_page: "repos",
        csrf_token,
        left_panel_repos,
        current_repo_id: None,
        webdav_base_url: state
            .config
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
