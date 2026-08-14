//! Web UI for wikis (Seafile wiki2). The mobile clients load `/wikis/{repo_id}/`
//! in a WebView, so this must render the navigation tree and page content
//! server-side.

use askama::Template;
use axum::Form;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use serde::Deserialize;
use std::sync::Arc;

use base::error::AppError;

use crate::AppState;
use crate::i18n::I18n;
use crate::markdown::render_markdown;
use crate::service::repo::service::LeftPanelRepo;

use super::auth_extractor::WebUser;

/// One row in the rendered navigation tree (flattened with indentation depth).
pub struct WikiNavItem {
    pub depth: usize,
    pub id: String,
    pub name: String,
    pub active: bool,
}

#[derive(Template)]
#[template(path = "wiki/page.html")]
pub struct WikiPageTemplate {
    pub urls: &'static crate::static_assets::TemplateUrls,
    pub t: &'static I18n,
    pub user_email: String,
    pub is_admin: bool,
    pub csrf_token: String,
    pub left_panel_repos: Vec<LeftPanelRepo>,
    pub active_page: &'static str,
    pub current_repo_id: Option<String>,
    pub repo_id: String,
    pub repo_name: String,
    pub nav_items: Vec<WikiNavItem>,
    pub page_id: String,
    pub page_name: String,
    pub content_html: String,
    pub can_edit: bool,
    pub publish_url: String,
}

#[derive(Template)]
#[template(path = "wiki/edit.html")]
pub struct WikiEditTemplate {
    pub urls: &'static crate::static_assets::TemplateUrls,
    pub t: &'static I18n,
    pub user_email: String,
    pub is_admin: bool,
    pub csrf_token: String,
    pub left_panel_repos: Vec<LeftPanelRepo>,
    pub active_page: &'static str,
    pub current_repo_id: Option<String>,
    pub repo_id: String,
    pub repo_name: String,
    pub page_id: String,
    pub page_name: String,
    pub content: String,
    pub cancel_url: String,
}

#[derive(Deserialize)]
pub struct WikiViewQuery {
    pub page_id: Option<String>,
}

#[derive(Deserialize)]
pub struct WikiSaveForm {
    pub csrf_token: Option<String>,
    pub content: Option<String>,
}

/// Build the shared template fields + repo access for a wiki page.
struct WikiPageContext {
    repo_id: String,
    repo_name: String,
    config: serde_json::Value,
    can_edit: bool,
    publish_url: String,
}

async fn load_wiki_context(
    state: &AppState,
    user: &WebUser,
    repo_id: &str,
) -> Result<WikiPageContext, AppError> {
    let svc = state.wiki_service();
    let repo = state
        .repos
        .repo
        .find_by_id(repo_id)
        .await?
        .ok_or_else(|| AppError::NotFound("wiki not found".into()))?;
    if repo.r#type != "wiki" {
        return Err(AppError::NotFound("wiki not found".into()));
    }
    crate::domain::permission::check_repo_read_permission(
        state.repos.member.as_ref(),
        repo_id,
        user.user_id,
    )
    .await?;

    let config = svc.read_wiki_config(repo_id).await?;
    let can_edit = crate::domain::permission::check_repo_write_permission(
        state.repos.member.as_ref(),
        repo_id,
        user.user_id,
    )
    .await
    .is_ok();
    let publish_url = state
        .repos
        .wiki2_publish
        .find_by_repo_id(repo_id)
        .await?
        .map(|p| p.publish_url)
        .unwrap_or_default();

    Ok(WikiPageContext {
        repo_id: repo_id.to_string(),
        repo_name: repo.name.clone(),
        config,
        can_edit,
        publish_url,
    })
}

/// Resolve the active page id: the `page_id` query param, else the first page
/// in the config (usually `home`).
fn resolve_page_id(config: &serde_json::Value, requested: Option<&str>) -> Option<String> {
    if let Some(id) = requested.filter(|id| find_page(config, id).is_some()) {
        return Some(id.to_string());
    }
    config
        .get("pages")
        .and_then(|p| p.as_array())
        .and_then(|arr| arr.first())
        .and_then(|p| p.get("id").and_then(|v| v.as_str()))
        .map(|s| s.to_string())
}

use crate::service::wiki::find_page;

/// Flatten the navigation tree into a display list.
fn build_nav(
    navigation: &serde_json::Value,
    config: &serde_json::Value,
    active_id: &str,
    depth: usize,
    out: &mut Vec<WikiNavItem>,
) {
    let Some(arr) = navigation.as_array() else {
        return;
    };
    for node in arr {
        let id = node.get("id").and_then(|v| v.as_str()).unwrap_or("");
        let name = page_name(config, id).unwrap_or_else(|| id.to_string());
        out.push(WikiNavItem {
            depth,
            id: id.to_string(),
            name,
            active: id == active_id,
        });
        if let Some(children) = node.get("children") {
            build_nav(children, config, active_id, depth + 1, out);
        }
    }
}

fn page_name(config: &serde_json::Value, id: &str) -> Option<String> {
    find_page(config, id)
        .and_then(|p| p.get("name").and_then(|v| v.as_str()))
        .map(|s| s.to_string())
}

/// `GET /wikis/{repo_id}/` — wiki view (navigation tree + rendered page).
pub async fn wiki_view(
    user: WebUser,
    State(state): State<Arc<AppState>>,
    Path(repo_id): Path<String>,
    Query(query): Query<WikiViewQuery>,
) -> Result<impl IntoResponse, AppError> {
    let ctx = load_wiki_context(&state, &user, &repo_id).await?;
    let svc = state.wiki_service();
    let page_id = resolve_page_id(&ctx.config, query.page_id.as_deref())
        .ok_or_else(|| AppError::NotFound("no wiki pages".into()))?;
    let page_name = find_page(&ctx.config, &page_id)
        .and_then(|p| p.get("name").and_then(|v| v.as_str()))
        .unwrap_or(&page_id)
        .to_string();

    let content = svc
        .get_page_content_from_config(&repo_id, &page_id, &ctx.config)
        .await?
        .map(|(_, bytes)| String::from_utf8_lossy(&bytes).into_owned())
        .unwrap_or_default();
    let content_html = render_markdown(&content);

    let mut nav_items = Vec::new();
    build_nav(
        &ctx.config["navigation"],
        &ctx.config,
        &page_id,
        0,
        &mut nav_items,
    );

    let page_ctx = crate::ui::ctx::build_page_ctx(&state, &user).await?;
    let tpl = WikiPageTemplate {
        urls: page_ctx.urls,
        t: page_ctx.t,
        user_email: page_ctx.user_email,
        is_admin: page_ctx.is_admin,
        csrf_token: page_ctx.csrf_token,
        left_panel_repos: page_ctx.left_panel_repos,
        active_page: "wiki",
        current_repo_id: Some(ctx.repo_id.clone()),
        repo_id: ctx.repo_id.clone(),
        repo_name: ctx.repo_name,
        nav_items,
        page_id,
        page_name,
        content_html,
        can_edit: ctx.can_edit,
        publish_url: ctx.publish_url,
    };
    let html = tpl
        .render()
        .map_err(|e| AppError::internal(e.to_string()))?;
    Ok(axum::response::Html(html))
}

/// `GET /wikis/{repo_id}/page/{page_id}/edit/` — markdown edit form.
pub async fn wiki_page_edit(
    user: WebUser,
    State(state): State<Arc<AppState>>,
    Path((repo_id, page_id)): Path<(String, String)>,
) -> Result<impl IntoResponse, AppError> {
    let ctx = load_wiki_context(&state, &user, &repo_id).await?;
    if !ctx.can_edit {
        return Err(AppError::Forbidden);
    }
    let svc = state.wiki_service();
    let page_name = find_page(&ctx.config, &page_id)
        .and_then(|p| p.get("name").and_then(|v| v.as_str()))
        .unwrap_or(&page_id)
        .to_string();
    let content = svc
        .get_page_content(&repo_id, &page_id)
        .await?
        .map(|(_, bytes)| String::from_utf8_lossy(&bytes).into_owned())
        .unwrap_or_default();

    let page_ctx = crate::ui::ctx::build_page_ctx(&state, &user).await?;
    let tpl = WikiEditTemplate {
        urls: page_ctx.urls,
        t: page_ctx.t,
        user_email: page_ctx.user_email,
        is_admin: page_ctx.is_admin,
        csrf_token: page_ctx.csrf_token,
        left_panel_repos: page_ctx.left_panel_repos,
        active_page: "wiki",
        current_repo_id: Some(ctx.repo_id.clone()),
        repo_id: ctx.repo_id.clone(),
        repo_name: ctx.repo_name,
        page_id,
        page_name,
        content,
        cancel_url: format!("/wikis/{}/", ctx.repo_id),
    };
    let html = tpl
        .render()
        .map_err(|e| AppError::internal(e.to_string()))?;
    Ok(axum::response::Html(html))
}

/// `POST /wikis/{repo_id}/page/{page_id}/save/` — save markdown content.
pub async fn wiki_page_save(
    user: WebUser,
    State(state): State<Arc<AppState>>,
    Path((repo_id, page_id)): Path<(String, String)>,
    Form(form): Form<WikiSaveForm>,
) -> Result<impl IntoResponse, AppError> {
    crate::service::auth::csrf::check_form_csrf(
        &state,
        &user.session_token,
        form.csrf_token.as_deref(),
    )?;
    let content = form.content.unwrap_or_default();
    state
        .wiki_service()
        .save_page_content(
            &repo_id,
            user.user_id,
            &user.email,
            &page_id,
            content.as_bytes(),
        )
        .await?;

    Ok((
        StatusCode::FOUND,
        [("Location", format!("/wikis/{repo_id}/?page_id={page_id}"))],
    )
        .into_response())
}
