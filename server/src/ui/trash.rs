use askama::Template;
use axum::{
    extract::{Query, State},
    response::Html,
};
use std::sync::Arc;

use crate::AppState;
use crate::fs::core::trash;
use crate::i18n::I18n;
use crate::ui::files::format_size;
use base::error::AppError;

use super::auth_extractor::WebUser;

// ─── Query ───────────────────────────────────────────────────────────────

#[derive(serde::Deserialize)]
pub struct TrashQuery {
    pub page: Option<u32>,
    pub per_page: Option<u32>,
    pub q: Option<String>,
    pub tab: Option<String>,
    pub restored: Option<usize>,
    pub failed: Option<usize>,
    pub cleaned: Option<bool>,
    pub lib_restored: Option<bool>,
    pub lib_deleted: Option<bool>,
    pub libs_deleted: Option<bool>,
}

// ─── Template ────────────────────────────────────────────────────────────

#[derive(Template)]
#[template(path = "trash/list.html")]
pub struct TrashListTemplate {
    pub urls: &'static crate::static_assets::TemplateUrls,
    pub t: &'static I18n,
    pub user_email: String,
    pub is_admin: bool,
    pub items: Vec<TrashEntryView>,
    /// Libraries in the caller's trash, newest deletion first.
    pub deleted_repos: Vec<DeletedRepoView>,
    pub total_count: i64,
    pub current_page: u32,
    pub per_page: u32,
    pub total_pages: u32,
    pub query: String,
    pub restored: usize,
    pub failed: usize,
    pub cleaned: bool,
    pub lib_restored: bool,
    pub lib_deleted: bool,
    pub libs_deleted: bool,
    pub active_page: &'static str,
    pub active_tab: String,
    pub csrf_token: String,
    pub left_panel_repos: Vec<crate::service::repo::service::LeftPanelRepo>,
    pub current_repo_id: Option<String>,
}

pub struct TrashEntryView {
    pub obj_name: String,
    pub parent_dir: String,
    pub deleted_time_display: String,
    pub deleted_time_ts: i64,
    pub commit_id: String,
    pub is_dir: bool,
    pub size_display: String,
    pub repo_id: String,
    pub repo_name: String,
}

pub struct DeletedRepoView {
    pub repo_id: String,
    pub repo_name: String,
    pub size_display: String,
    pub deleted_time_ts: i64,
}

// ─── Handlers ───────────────────────────────────────────────────────────

/// GET /trash/ — global trash listing across all accessible repos.
pub async fn trash_list_page(
    user: WebUser,
    State(state): State<Arc<AppState>>,
    Query(query): Query<TrashQuery>,
) -> Result<Html<String>, AppError> {
    let db = state.db.as_ref();

    let page = query.page.unwrap_or(1).max(1);
    let per_page = query.per_page.unwrap_or(50).clamp(1, 100);
    let q = query.q.as_deref().unwrap_or("").to_string();

    // Fetch trash items across all accessible repos
    let result = if q.is_empty() {
        trash::list_trash_for_user(db, &state.repos, user.user_id, page, per_page).await?
    } else {
        trash::search_trash_for_user(
            db,
            &state.repos,
            user.user_id,
            &q,
            page,
            per_page,
            None,
            None,
            None,
        )
        .await?
    };

    // Format items for display
    let items: Vec<TrashEntryView> = result
        .items
        .into_iter()
        .map(|entry| {
            let deleted_time_display = chrono::DateTime::parse_from_rfc3339(&entry.deleted_time)
                .map(|dt| dt.format("%Y-%m-%d %H:%M").to_string())
                .unwrap_or_else(|_| entry.deleted_time.clone());

            TrashEntryView {
                obj_name: entry.obj_name,
                parent_dir: entry.parent_dir,
                deleted_time_display,
                deleted_time_ts: entry.deleted_time_ts,
                commit_id: entry.commit_id,
                is_dir: entry.is_dir,
                size_display: format_size(entry.size),
                repo_id: entry.repo_id,
                repo_name: entry.repo_name,
            }
        })
        .collect();

    let total_count = result.total_count;
    let total_pages = if per_page > 0 {
        ((total_count as f64) / (per_page as f64)).ceil() as u32
    } else {
        1
    };
    let restored = query.restored.unwrap_or(0);
    let failed = query.failed.unwrap_or(0);
    let cleaned = query.cleaned.unwrap_or(false);

    // Deleted libraries live in their own tab; the list is per owner and short
    // (one row per deleted library), so it is always loaded for the tab count.
    let deleted_repos: Vec<DeletedRepoView> = trash::list_deleted_repos(&state.repos, user.user_id)
        .await?
        .into_iter()
        .map(|r| DeletedRepoView {
            repo_id: r.repo_id,
            repo_name: r.repo_name,
            size_display: format_size(r.size),
            deleted_time_ts: r.del_time,
        })
        .collect();

    let active_tab = query
        .tab
        .filter(|t| t == "libraries")
        .unwrap_or_else(|| "files".to_string());

    let ctx = crate::ui::ctx::build_page_ctx(&state, &user).await?;

    let tpl = TrashListTemplate {
        urls: ctx.urls,
        t: ctx.t,
        user_email: ctx.user_email,
        is_admin: ctx.is_admin,
        items,
        deleted_repos,
        total_count,
        current_page: page,
        per_page,
        total_pages,
        query: q,
        restored,
        failed,
        cleaned,
        lib_restored: query.lib_restored.unwrap_or(false),
        lib_deleted: query.lib_deleted.unwrap_or(false),
        libs_deleted: query.libs_deleted.unwrap_or(false),
        active_page: "trash",
        active_tab,
        csrf_token: ctx.csrf_token,
        left_panel_repos: ctx.left_panel_repos,
        current_repo_id: None,
    };

    let html = tpl
        .render()
        .map_err(|e| AppError::Internal(e.to_string()))?;
    Ok(Html(html))
}
