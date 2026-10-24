use askama::Template;
use axum::{
    extract::{Query, State},
    response::Html,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

use crate::AppState;
use crate::domain::permission::RepoScope;
use crate::i18n::I18n;
use crate::service::fs::search::{FileSearchResult, SearchRequest};
use crate::ui::files::format_size;
use base::error::AppError;

use super::auth_extractor::WebUser;

#[derive(Template)]
#[template(path = "search.html")]
pub struct SearchTemplate {
    pub urls: &'static crate::static_assets::TemplateUrls,
    pub t: &'static I18n,
    pub user_email: String,
    pub is_admin: bool,
    pub query: String,
    pub active_page: &'static str,
    pub results: Vec<SearchResultItem>,
    pub total: i32,
    pub has_more: bool,
    pub per_page: i32,
    pub current_page: i32,
    pub search_filename_only: bool,
    pub left_panel_repos: Vec<crate::service::repo::service::LeftPanelRepo>,
    pub sharing_enabled: bool,
    pub current_repo_id: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct SearchResultItem {
    pub repo_id: String,
    pub repo_name: String,
    pub name: String,
    pub oid: String,
    pub last_modified: i64,
    #[serde(skip)]
    pub size_display: String,
    pub fullpath: String,
    pub size: i64,
    pub is_dir: bool,
    /// URL for the directory containing this file (or the directory itself).
    #[serde(skip)]
    pub dir_url: String,
    /// HTML snippet with `<mark>` highlighting; empty for a filename-only match.
    pub content_highlight: String,
}

impl From<FileSearchResult> for SearchResultItem {
    fn from(hit: FileSearchResult) -> Self {
        Self {
            repo_id: hit.repo_id,
            repo_name: hit.repo_name,
            name: hit.name,
            oid: hit.oid,
            last_modified: hit.last_modified,
            // Only the page renders a human-readable size; the JSON endpoints send bytes.
            size_display: format_size(hit.size),
            fullpath: hit.fullpath,
            size: hit.size,
            is_dir: hit.is_dir,
            dir_url: hit.dir_url,
            content_highlight: hit.content_highlight,
        }
    }
}

#[derive(Deserialize)]
pub struct SearchQuery {
    pub q: Option<String>,
    /// When true, search filenames only; absent or false searches content too.
    pub search_filename_only: Option<bool>,
}

/// GET /search?q=xxx — search page (Web UI).
///
/// The page renders the first page itself and appends further pages from `/api2/search/`, so it
/// shares the one [`crate::service::fs::search::SearchService`] pipeline with that endpoint instead
/// of walking the tree a second way.
pub async fn search_page(
    user: WebUser,
    State(state): State<Arc<AppState>>,
    Query(query): Query<SearchQuery>,
) -> Result<Html<String>, AppError> {
    let q = query.q.unwrap_or_default();
    let search_filename_only = query.search_filename_only.unwrap_or(false);
    let per_page: i32 = 20;
    let page: i32 = 1;

    let hits = if q.trim().is_empty() {
        Vec::new()
    } else {
        state
            .search_service()
            .collect_hits(&SearchRequest {
                q: &q,
                user_id: user.user_id,
                search_repo: None,
                filename_only: search_filename_only,
                // A browser session is not narrowed by a bound API key.
                repo_scope: &RepoScope::all(),
            })
            .await?
    };

    let total = hits.len() as i32;
    let has_more = total > per_page;
    let results: Vec<SearchResultItem> = hits
        .into_iter()
        .take(per_page as usize)
        .map(SearchResultItem::from)
        .collect();

    let ctx = crate::ui::ctx::build_page_ctx(&state, &user).await?;
    let tpl = SearchTemplate {
        urls: ctx.urls,
        t: ctx.t,
        user_email: ctx.user_email,
        is_admin: ctx.is_admin,
        query: q,
        active_page: "search",
        results,
        total,
        has_more,
        per_page,
        current_page: page,
        search_filename_only,
        left_panel_repos: ctx.left_panel_repos,
        sharing_enabled: ctx.sharing_enabled,
        current_repo_id: None,
    };
    let html = tpl
        .render()
        .map_err(|e| AppError::internal(e.to_string()))?;
    Ok(Html(html))
}
