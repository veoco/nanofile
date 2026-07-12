/// Web UI repo handlers — list repos.
use askama::Template;
use axum::{extract::State, response::Html};
use std::sync::Arc;

use crate::AppState;
use crate::ui::files::format_size;
use base::error::AppError;

use super::auth_extractor::WebUser;

// ─── Templates ───────────────────────────────────────────────────────────────

#[derive(Template)]
#[template(path = "repos/list.html")]
pub struct RepoListTemplate {
    pub urls: &'static crate::static_assets::TemplateUrls,
    pub user_email: String,
    pub is_admin: bool,
    pub repos: Vec<RepoInfo>,
    pub active_page: &'static str,
    pub user_id: i32,
    pub csrf_token: String,
    pub left_panel_repos: Vec<crate::service::repo::service::LeftPanelRepo>,
    pub current_repo_id: Option<String>,
}

// ─── Data types ──────────────────────────────────────────────────────────────

pub struct RepoInfo {
    pub id: String,
    pub name: String,
    pub description: String,
    pub size: i64,
    pub size_display: String,
    pub mtime: i64,
    pub encrypted: bool,
    pub owner_id: i32,
}

// ─── Handlers ────────────────────────────────────────────────────────────────

/// GET /libraries/ — list user's repos.
pub async fn list_repos(
    user: WebUser,
    State(state): State<Arc<AppState>>,
) -> Result<Html<String>, AppError> {
    // Find repos where user is a member
    let memberships = state.repos.member.find_by_user_id(user.user_id).await?;

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
                owner_id: r.owner_id,
            });
        }
    }

    let csrf_token =
        crate::service::auth::csrf::generate_csrf_token(&state.csrf_secret, &user.session_token);

    let left_panel_repos: Vec<crate::service::repo::service::LeftPanelRepo> = repos
        .iter()
        .map(|r| crate::service::repo::service::LeftPanelRepo {
            id: r.id.clone(),
            name: r.name.clone(),
            size_display: r.size_display.clone(),
        })
        .collect();

    let tpl = RepoListTemplate {
        urls: crate::static_assets::template_urls(),
        user_email: user.email,
        is_admin: user.is_admin,
        repos,
        active_page: "repos",
        user_id: user.user_id,
        csrf_token,
        left_panel_repos,
        current_repo_id: None,
    };

    let html = tpl
        .render()
        .map_err(|e| AppError::internal(e.to_string()))?;
    Ok(Html(html))
}
