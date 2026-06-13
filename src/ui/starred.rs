/// Web UI starred-items page.
use askama::Template;
use axum::{
    extract::State,
    response::{Html, IntoResponse, Redirect},
};
use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};
use std::sync::Arc;

use crate::AppState;
use crate::entity::{repo, starred_file};
use crate::error::AppError;

use super::auth_extractor::WebUser;

#[derive(Template)]
#[template(path = "starred/list.html")]
pub struct StarredTemplate {
    pub urls: &'static crate::static_assets::TemplateUrls,
    pub user_email: String,
    pub is_admin: bool,
    pub starred_repos: Vec<StarredItemView>,
    pub starred_folders: Vec<StarredItemView>,
    pub starred_files: Vec<StarredItemView>,
    pub active_page: &'static str,
}

pub struct StarredItemView {
    pub repo_id: String,
    pub repo_name: String,
    pub path: String,
    pub obj_name: String,
    pub is_dir: bool,
    pub mtime_display: String,
    pub deleted: bool,
}

/// GET /starred/ — list all starred items.
pub async fn starred_page(
    user: WebUser,
    State(state): State<Arc<AppState>>,
) -> Result<Html<String>, AppError> {
    let db = state.db.as_ref();

    let entries = starred_file::Entity::find()
        .filter(starred_file::Column::UserId.eq(user.user_id))
        .all(db)
        .await?;

    // Cache repo lookups to avoid N+1
    let mut repo_cache: std::collections::HashMap<String, Option<repo::Model>> =
        std::collections::HashMap::new();
    for entry in &entries {
        if !repo_cache.contains_key(&entry.repo_id) {
            let r = repo::Entity::find_by_id(&entry.repo_id).one(db).await?;
            repo_cache.insert(entry.repo_id.clone(), r);
        }
    }

    let mut starred_repos = Vec::new();
    let mut starred_folders = Vec::new();
    let mut starred_files = Vec::new();

    for entry in &entries {
        let (repo_name, deleted) = match repo_cache.get(&entry.repo_id).and_then(|o| o.as_ref()) {
            Some(r) => (r.name.clone(), false),
            None => (String::new(), true),
        };

        let obj_name = if entry.path == "/" {
            repo_name.clone()
        } else {
            entry
                .path
                .trim_end_matches('/')
                .rsplit_once('/')
                .map(|(_, n)| n.to_string())
                .unwrap_or_default()
        };

        let view = StarredItemView {
            repo_id: entry.repo_id.clone(),
            repo_name,
            path: entry.path.clone(),
            obj_name,
            is_dir: entry.is_dir,
            mtime_display: crate::ui::files::format_mtime(entry.created_at),
            deleted,
        };

        if entry.path == "/" {
            starred_repos.push(view);
        } else if entry.is_dir {
            starred_folders.push(view);
        } else {
            starred_files.push(view);
        }
    }

    // Sort by mtime descending (most recently starred first)
    let sort_desc =
        |a: &StarredItemView, b: &StarredItemView| b.mtime_display.cmp(&a.mtime_display);
    starred_repos.sort_by(sort_desc);
    starred_folders.sort_by(sort_desc);
    starred_files.sort_by(sort_desc);

    let tpl = StarredTemplate {
        urls: crate::static_assets::template_urls(),
        user_email: user.email,
        is_admin: user.is_admin,
        starred_repos,
        starred_folders,
        starred_files,
        active_page: "starred",
    };

    let html = tpl
        .render()
        .map_err(|e| AppError::internal(e.to_string()))?;
    Ok(Html(html))
}

/// POST /starred/ — unstar an item (form submission from the starred page).
pub async fn unstar_item_ui(
    user: WebUser,
    State(state): State<Arc<AppState>>,
    axum::Form(form): axum::Form<std::collections::HashMap<String, String>>,
) -> Result<impl IntoResponse, AppError> {
    let db = state.db.as_ref();

    let repo_id = form
        .get("repo_id")
        .ok_or_else(|| AppError::BadRequest("repo_id required".into()))?;
    let path = form
        .get("path")
        .ok_or_else(|| AppError::BadRequest("path required".into()))?;

    starred_file::Entity::delete_many()
        .filter(starred_file::Column::UserId.eq(user.user_id))
        .filter(starred_file::Column::RepoId.eq(repo_id))
        .filter(starred_file::Column::Path.eq(path))
        .exec(db)
        .await?;

    Ok(Redirect::to("/starred/"))
}
