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
    pub breadcrumb: Vec<WikiNavItem>,
    pub page_id: String,
    pub page_name: String,
    pub page_locked: bool,
    pub content_html: String,
    pub can_edit: bool,
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

/// One wiki in the list page (typed from the `wikis` array of
/// `WikiService::list_wikis_v2`).
pub struct WikiListItem {
    pub id: String,
    pub name: String,
}

#[derive(Template)]
#[template(path = "wiki/list.html")]
pub struct WikiListTemplate {
    pub urls: &'static crate::static_assets::TemplateUrls,
    pub t: &'static I18n,
    pub user_email: String,
    pub is_admin: bool,
    pub csrf_token: String,
    pub left_panel_repos: Vec<LeftPanelRepo>,
    pub active_page: &'static str,
    pub current_repo_id: Option<String>,
    pub wikis: Vec<WikiListItem>,
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

/// Shared CSRF-bearing form for name-based wiki actions (create / rename).
#[derive(Deserialize)]
pub struct WikiNameForm {
    pub csrf_token: Option<String>,
    pub name: Option<String>,
}

/// CSRF-only form for delete actions.
#[derive(Deserialize)]
pub struct WikiCsrfForm {
    pub csrf_token: Option<String>,
}

#[derive(Deserialize)]
pub struct WikiPageCreateForm {
    pub csrf_token: Option<String>,
    pub page_name: Option<String>,
    pub current_id: Option<String>,
    pub insert_position: Option<String>,
}

#[derive(Deserialize)]
pub struct WikiPageRenameForm {
    pub csrf_token: Option<String>,
    pub page_name: Option<String>,
}

#[derive(Deserialize)]
pub struct WikiPageMoveForm {
    pub csrf_token: Option<String>,
    pub target_id: Option<String>,
    pub move_position: Option<String>,
}

#[derive(Deserialize)]
pub struct WikiPageLockForm {
    pub csrf_token: Option<String>,
    pub locked: Option<String>,
}

/// Build the shared template fields + repo access for a wiki page.
struct WikiPageContext {
    repo_id: String,
    repo_name: String,
    config: serde_json::Value,
    can_edit: bool,
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

    Ok(WikiPageContext {
        repo_id: repo_id.to_string(),
        repo_name: repo.name.clone(),
        config,
        can_edit,
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

/// Walk the navigation tree to collect the ancestor chain (root → active page).
/// Returns true once `active_id` is found; `out` then holds the full path.
fn find_breadcrumb(
    navigation: &serde_json::Value,
    config: &serde_json::Value,
    active_id: &str,
    out: &mut Vec<WikiNavItem>,
) -> bool {
    let Some(arr) = navigation.as_array() else {
        return false;
    };
    for node in arr {
        let id = node.get("id").and_then(|v| v.as_str()).unwrap_or("");
        let name = page_name(config, id).unwrap_or_else(|| id.to_string());
        out.push(WikiNavItem {
            depth: out.len(),
            id: id.to_string(),
            name,
            active: id == active_id,
        });
        if id == active_id {
            return true;
        }
        if let Some(children) = node.get("children")
            && find_breadcrumb(children, config, active_id, out)
        {
            return true;
        }
        out.pop();
    }
    false
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
    let page_locked = find_page(&ctx.config, &page_id)
        .and_then(|p| p.get("locked").and_then(|v| v.as_bool()))
        .unwrap_or(false);

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
    let mut breadcrumb = Vec::new();
    find_breadcrumb(
        &ctx.config["navigation"],
        &ctx.config,
        &page_id,
        &mut breadcrumb,
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
        breadcrumb,
        page_id,
        page_name,
        page_locked,
        content_html,
        can_edit: ctx.can_edit,
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

// ── Wiki list & management (server-rendered forms) ──────────────────────────

/// Build a 302 redirect (matching seahub's `HttpResponseRedirect`, which the
/// rest of the web UI uses; axum's `Redirect::to` would return 303).
fn redirect_to(url: impl Into<String>) -> axum::response::Response {
    (StatusCode::FOUND, [("Location", url.into())]).into_response()
}

/// Extract one typed list item from the `wikis` array of `list_wikis_v2`.
fn parse_wiki_item(v: &serde_json::Value) -> WikiListItem {
    let s = |k: &str| v.get(k).and_then(|x| x.as_str()).unwrap_or("").to_string();
    WikiListItem {
        id: s("id"),
        name: s("name"),
    }
}

/// `GET /wikis/` — list the user's wikis.
pub async fn wiki_list(
    user: WebUser,
    State(state): State<Arc<AppState>>,
) -> Result<impl IntoResponse, AppError> {
    let data = state
        .wiki_service()
        .list_wikis_v2(user.user_id, &user.email)
        .await?;
    let wikis = data
        .get("wikis")
        .and_then(|w| w.as_array())
        .map(|arr| arr.iter().map(parse_wiki_item).collect())
        .unwrap_or_default();

    let page_ctx = crate::ui::ctx::build_page_ctx(&state, &user).await?;
    let tpl = WikiListTemplate {
        urls: page_ctx.urls,
        t: page_ctx.t,
        user_email: page_ctx.user_email,
        is_admin: page_ctx.is_admin,
        csrf_token: page_ctx.csrf_token,
        left_panel_repos: page_ctx.left_panel_repos,
        active_page: "wiki",
        current_repo_id: None,
        wikis,
    };
    let html = tpl
        .render()
        .map_err(|e| AppError::internal(e.to_string()))?;
    Ok(axum::response::Html(html))
}

/// `POST /wikis/new/` — create a wiki, then redirect to the list.
pub async fn wiki_create(
    user: WebUser,
    State(state): State<Arc<AppState>>,
    Form(form): Form<WikiNameForm>,
) -> Result<impl IntoResponse, AppError> {
    crate::service::auth::csrf::check_form_csrf(
        &state,
        &user.session_token,
        form.csrf_token.as_deref(),
    )?;
    state
        .wiki_service()
        .create_wiki(user.user_id, &user.email, &form.name.unwrap_or_default())
        .await?;
    Ok(redirect_to("/wikis/"))
}

/// `POST /wikis/{repo_id}/rename/` — rename a wiki.
pub async fn wiki_rename(
    user: WebUser,
    State(state): State<Arc<AppState>>,
    Path(repo_id): Path<String>,
    Form(form): Form<WikiNameForm>,
) -> Result<impl IntoResponse, AppError> {
    crate::service::auth::csrf::check_form_csrf(
        &state,
        &user.session_token,
        form.csrf_token.as_deref(),
    )?;
    state
        .wiki_service()
        .rename_wiki(&repo_id, user.user_id, &form.name.unwrap_or_default())
        .await?;
    Ok(redirect_to("/wikis/"))
}

/// `POST /wikis/{repo_id}/delete/` — delete a wiki.
pub async fn wiki_delete(
    user: WebUser,
    State(state): State<Arc<AppState>>,
    Path(repo_id): Path<String>,
    Form(form): Form<WikiCsrfForm>,
) -> Result<impl IntoResponse, AppError> {
    crate::service::auth::csrf::check_form_csrf(
        &state,
        &user.session_token,
        form.csrf_token.as_deref(),
    )?;
    state
        .wiki_service()
        .delete_wiki(&repo_id, user.user_id)
        .await?;
    Ok(redirect_to("/wikis/"))
}

// ── Wiki page management (server-rendered forms) ────────────────────────────

/// `POST /wikis/{repo_id}/page/new/` — create a page, redirect to it.
pub async fn wiki_page_create(
    user: WebUser,
    State(state): State<Arc<AppState>>,
    Path(repo_id): Path<String>,
    Form(form): Form<WikiPageCreateForm>,
) -> Result<impl IntoResponse, AppError> {
    crate::service::auth::csrf::check_form_csrf(
        &state,
        &user.session_token,
        form.csrf_token.as_deref(),
    )?;
    let result = state
        .wiki_service()
        .create_page(
            &repo_id,
            user.user_id,
            &user.email,
            &form.page_name.unwrap_or_default(),
            form.current_id.as_deref(),
            form.insert_position.as_deref(),
        )
        .await?;
    let new_page_id = result
        .get("file_info")
        .and_then(|f| f.get("page_id"))
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let target = if new_page_id.is_empty() {
        format!("/wikis/{repo_id}/")
    } else {
        format!("/wikis/{repo_id}/?page_id={new_page_id}")
    };
    Ok(redirect_to(target))
}

/// `POST /wikis/{repo_id}/page/{page_id}/delete/` — delete a page.
pub async fn wiki_page_delete(
    user: WebUser,
    State(state): State<Arc<AppState>>,
    Path((repo_id, page_id)): Path<(String, String)>,
    Form(form): Form<WikiCsrfForm>,
) -> Result<impl IntoResponse, AppError> {
    crate::service::auth::csrf::check_form_csrf(
        &state,
        &user.session_token,
        form.csrf_token.as_deref(),
    )?;
    state
        .wiki_service()
        .delete_page(&repo_id, user.user_id, &user.email, &page_id)
        .await?;
    Ok(redirect_to(format!("/wikis/{repo_id}/")))
}

/// `POST /wikis/{repo_id}/page/{page_id}/rename/` — rename a page.
pub async fn wiki_page_rename(
    user: WebUser,
    State(state): State<Arc<AppState>>,
    Path((repo_id, page_id)): Path<(String, String)>,
    Form(form): Form<WikiPageRenameForm>,
) -> Result<impl IntoResponse, AppError> {
    crate::service::auth::csrf::check_form_csrf(
        &state,
        &user.session_token,
        form.csrf_token.as_deref(),
    )?;
    state
        .wiki_service()
        .update_page_config(
            &repo_id,
            user.user_id,
            &user.email,
            &page_id,
            form.page_name.as_deref(),
            None,
            None,
        )
        .await?;
    Ok(redirect_to(format!("/wikis/{repo_id}/?page_id={page_id}")))
}

/// `POST /wikis/{repo_id}/page/{page_id}/move/` — move a page in the navigation.
pub async fn wiki_page_move(
    user: WebUser,
    State(state): State<Arc<AppState>>,
    Path((repo_id, page_id)): Path<(String, String)>,
    Form(form): Form<WikiPageMoveForm>,
) -> Result<impl IntoResponse, AppError> {
    crate::service::auth::csrf::check_form_csrf(
        &state,
        &user.session_token,
        form.csrf_token.as_deref(),
    )?;
    let target_id = form.target_id.unwrap_or_default();
    let move_position = form.move_position.unwrap_or_default();
    state
        .wiki_service()
        .move_page(
            &repo_id,
            user.user_id,
            &user.email,
            &target_id,
            &page_id,
            &move_position,
        )
        .await?;
    Ok(redirect_to(format!("/wikis/{repo_id}/?page_id={page_id}")))
}

/// `POST /wikis/{repo_id}/page/{page_id}/lock/` — lock / unlock a page.
pub async fn wiki_page_lock(
    user: WebUser,
    State(state): State<Arc<AppState>>,
    Path((repo_id, page_id)): Path<(String, String)>,
    Form(form): Form<WikiPageLockForm>,
) -> Result<impl IntoResponse, AppError> {
    crate::service::auth::csrf::check_form_csrf(
        &state,
        &user.session_token,
        form.csrf_token.as_deref(),
    )?;
    let locked = form
        .locked
        .as_deref()
        .is_some_and(|v| v == "true" || v == "on" || v == "1");
    state
        .wiki_service()
        .set_page_locked(&repo_id, user.user_id, &user.email, &page_id, locked)
        .await?;
    Ok(redirect_to(format!("/wikis/{repo_id}/?page_id={page_id}")))
}
