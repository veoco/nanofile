use askama::Template;
use axum::{
    extract::{Query, State},
    response::Html,
};
use std::sync::Arc;

use crate::AppState;
use crate::fs::core::trash;
use crate::ui::files::format_size;
use base::error::AppError;

use super::auth_extractor::WebUser;

// ─── Query ───────────────────────────────────────────────────────────────

#[derive(serde::Deserialize)]
pub struct TrashQuery {
    pub page: Option<u32>,
    pub per_page: Option<u32>,
    pub q: Option<String>,
    pub restored: Option<usize>,
    pub failed: Option<usize>,
    pub cleaned: Option<bool>,
}

// ─── Template ────────────────────────────────────────────────────────────

#[derive(Template)]
#[template(path = "trash/list.html")]
pub struct TrashListTemplate {
    pub urls: &'static crate::static_assets::TemplateUrls,
    pub user_email: String,
    pub is_admin: bool,
    pub items: Vec<TrashEntryView>,
    pub total_count: i64,
    pub current_page: u32,
    pub per_page: u32,
    pub total_pages: u32,
    pub query: String,
    pub restored: usize,
    pub failed: usize,
    pub cleaned: bool,
    pub active_page: &'static str,
    pub csrf_token: String,
    pub left_panel_repos: Vec<crate::service::repo::service::LeftPanelRepo>,
    pub current_repo_id: Option<String>,
}

pub struct TrashEntryView {
    pub obj_name: String,
    pub parent_dir: String,
    pub deleted_time_display: String,
    pub commit_id: String,
    pub is_dir: bool,
    pub size_display: String,
    pub repo_id: String,
    pub repo_name: String,
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

    let csrf_token =
        crate::service::auth::csrf::generate_csrf_token(&state.csrf_secret, &user.session_token);
    let left_panel_repos =
        crate::service::repo::service::load_left_panel_repos(&state.repos, user.user_id).await?;

    let tpl = TrashListTemplate {
        urls: crate::static_assets::template_urls(),
        user_email: user.email.clone(),
        is_admin: user.is_admin,
        items,
        total_count,
        current_page: page,
        per_page,
        total_pages,
        query: q,
        restored,
        failed,
        cleaned,
        active_page: "trash",
        csrf_token,
        left_panel_repos,
        current_repo_id: None,
    };

    let html = tpl
        .render()
        .map_err(|e| AppError::Internal(e.to_string()))?;
    Ok(Html(html))
}
