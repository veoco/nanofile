/// Web UI search handler.
use askama::Template;
use axum::{
    extract::{Query, State},
    response::Html,
};
use sea_orm::{ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

use crate::AppState;
use crate::error::AppError;
use crate::ui::files::format_size;

use super::auth_extractor::WebUser;

#[derive(Template)]
#[template(path = "search.html")]
pub struct SearchTemplate {
    pub urls: &'static crate::static_assets::TemplateUrls,
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
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct SearchResultItem {
    pub repo_id: String,
    pub repo_name: String,
    pub name: String,
    pub oid: String,
    pub last_modified: i64,
    #[serde(skip)]
    pub last_modified_readable: String,
    #[serde(skip)]
    pub size_display: String,
    pub fullpath: String,
    pub size: i64,
    pub is_dir: bool,
}

#[derive(Deserialize)]
pub struct SearchQuery {
    pub q: Option<String>,
    /// When absent or true, search filenames only (default behavior).
    /// When false, also search file content via the full-text index.
    pub search_filename_only: Option<bool>,
}

/// GET /search?q=xxx — search page (Web UI).
pub async fn search_page(
    user: WebUser,
    State(state): State<Arc<AppState>>,
    Query(query): Query<SearchQuery>,
) -> Result<Html<String>, AppError> {
    let q = query.q.unwrap_or_default();
    let search_filename_only = query.search_filename_only.unwrap_or(true);
    let per_page: i32 = 20;
    let page: i32 = 1;

    let (results, total, has_more) = if q.trim().is_empty() {
        (Vec::new(), 0, false)
    } else {
        let db = state.db.as_ref();
        let repo_ids = get_accessible_repo_ids(db, user.user_id, None).await?;
        let mut seen = std::collections::HashSet::new();
        let mut all_results: Vec<SearchResultItem> = Vec::new();

        // Phase 1: Full-text search via Tantivy.
        if !search_filename_only && let Some(indexer) = &state.indexer {
            let ft_results = indexer.search(&q, &repo_ids, 200, 0).unwrap_or_default();
            for (found_repo_id, found_fullpath) in &ft_results {
                if !seen.insert((found_repo_id.clone(), found_fullpath.clone())) {
                    continue;
                }
                if let Some(item) = resolve_file_metadata(db, found_repo_id, found_fullpath).await {
                    all_results.push(item);
                }
            }
        }

        // Phase 2: Filename search (always).
        for repo_id in &repo_ids {
            let repo_record = match crate::entity::repo::Entity::find_by_id(repo_id)
                .one(db)
                .await
            {
                Ok(Some(r)) => r,
                _ => continue,
            };

            let head_commit_id = match &repo_record.head_commit_id {
                Some(id) => id.clone(),
                None => continue,
            };

            let head = match crate::entity::commit::Entity::find()
                .filter(crate::entity::commit::Column::RepoId.eq(repo_id))
                .filter(crate::entity::commit::Column::CommitId.eq(&head_commit_id))
                .one(db)
                .await
            {
                Ok(Some(h)) => h,
                _ => continue,
            };

            if head.root_id == "0000000000000000000000000000000000000000" {
                continue;
            }

            search_fs_tree(
                db,
                repo_id,
                &repo_record.name,
                &head.root_id,
                "",
                &q,
                &mut all_results,
                &mut seen,
            )
            .await;
        }

        // Sort: directories first, then by name.
        all_results.sort_by(|a, b| b.is_dir.cmp(&a.is_dir).then_with(|| a.name.cmp(&b.name)));

        let total = all_results.len() as i32;
        let offset = ((page - 1) * per_page) as usize;

        let results = if offset < all_results.len() {
            let end = (offset + per_page as usize).min(all_results.len());
            all_results[offset..end].to_vec()
        } else {
            Vec::new()
        };

        let has_more = (offset + per_page as usize) < all_results.len();
        (results, total, has_more)
    };

    let tpl = SearchTemplate {
        urls: crate::static_assets::template_urls(),
        user_email: user.email.clone(),
        is_admin: user.is_admin,
        query: q,
        active_page: "search",
        results,
        total,
        has_more,
        per_page,
        current_page: page,
        search_filename_only,
    };
    let html = tpl
        .render()
        .map_err(|e| AppError::internal(e.to_string()))?;
    Ok(Html(html))
}

/// Resolve file metadata from DB to build a SearchResultItem for a
/// (repo_id, fullpath) pair discovered by full-text search.
async fn resolve_file_metadata(
    db: &DatabaseConnection,
    repo_id: &str,
    fullpath: &str,
) -> Option<SearchResultItem> {
    let repo_record = crate::entity::repo::Entity::find_by_id(repo_id)
        .one(db)
        .await
        .ok()??;
    let head_commit_id = repo_record.head_commit_id.as_ref()?;
    let head = crate::entity::commit::Entity::find()
        .filter(crate::entity::commit::Column::RepoId.eq(repo_id))
        .filter(crate::entity::commit::Column::CommitId.eq(head_commit_id))
        .one(db)
        .await
        .ok()??;
    let root_id = &head.root_id;
    if root_id == "0000000000000000000000000000000000000000" {
        return None;
    }

    let segments: Vec<&str> = fullpath
        .trim_start_matches('/')
        .split('/')
        .filter(|s| !s.is_empty())
        .collect();

    let name = segments.last()?;
    let parent_path = if segments.len() <= 1 {
        "/"
    } else {
        let parent_segments = &segments[..segments.len() - 1];
        Box::leak(format!("/{}", parent_segments.join("/")).into_boxed_str())
    };

    let parent_fs_id = if parent_path == "/" {
        root_id.clone()
    } else {
        crate::storage::resolve_fs_id(db, repo_id, root_id, parent_path, None)
            .await
            .ok()?
    };

    let dir_data = crate::storage::read_fs_dir_data(db, repo_id, &parent_fs_id)
        .await
        .ok()?;
    let entry = dir_data.dirents.iter().find(|d| d.name == *name)?;

    let is_dir = entry.mode & crate::serialization::S_IFDIR != 0;
    Some(SearchResultItem {
        repo_id: repo_id.to_string(),
        repo_name: repo_record.name,
        name: entry.name.clone(),
        oid: entry.id.clone(),
        last_modified: entry.mtime,
        last_modified_readable: chrono::DateTime::from_timestamp(entry.mtime, 0)
            .map(|dt| dt.format("%Y-%m-%d %H:%M").to_string())
            .unwrap_or_else(|| entry.mtime.to_string()),
        size_display: format_size(entry.size),
        fullpath: fullpath.to_string(),
        size: entry.size,
        is_dir,
    })
}

async fn get_accessible_repo_ids(
    db: &DatabaseConnection,
    user_id: i32,
    _repo_id_filter: Option<&str>,
) -> Result<Vec<String>, AppError> {
    let member_repos = crate::entity::repo_member::Entity::find()
        .filter(crate::entity::repo_member::Column::UserId.eq(user_id))
        .all(db)
        .await?;

    let owned_repos = crate::entity::repo::Entity::find()
        .filter(crate::entity::repo::Column::OwnerId.eq(user_id))
        .all(db)
        .await?;

    let mut ids: Vec<String> = member_repos
        .into_iter()
        .map(|m| m.repo_id)
        .chain(owned_repos.into_iter().map(|r| r.id))
        .collect();

    ids.sort();
    ids.dedup();

    Ok(ids)
}

async fn search_fs_tree(
    db: &DatabaseConnection,
    repo_id: &str,
    repo_name: &str,
    root_fs_id: &str,
    base_path: &str,
    keyword: &str,
    results: &mut Vec<SearchResultItem>,
    seen: &mut std::collections::HashSet<(String, String)>,
) {
    let keyword_lower = keyword.to_lowercase();
    let mut stack: Vec<(String, String)> = vec![(root_fs_id.to_string(), base_path.to_string())];

    while let Some((fs_id, path)) = stack.pop() {
        if fs_id == "0000000000000000000000000000000000000000" {
            continue;
        }

        let dir_data = match crate::storage::read_fs_dir_data(db, repo_id, &fs_id).await {
            Ok(data) => data,
            Err(_) => continue,
        };

        for entry in &dir_data.dirents {
            let full_path = if path.is_empty() {
                format!("/{}", entry.name)
            } else if path.starts_with('/') {
                format!("{}/{}", path, entry.name)
            } else {
                format!("/{}/{}", path, entry.name)
            };

            if entry.name.to_lowercase().contains(&keyword_lower) {
                let key = (repo_id.to_string(), full_path.clone());
                if !seen.insert(key) {
                    continue; // Already seen from full-text search
                }
                let is_dir = entry.mode & crate::serialization::S_IFDIR != 0;
                results.push(SearchResultItem {
                    repo_id: repo_id.to_string(),
                    repo_name: repo_name.to_string(),
                    name: entry.name.clone(),
                    oid: entry.id.clone(),
                    last_modified: entry.mtime,
                    last_modified_readable: chrono::DateTime::from_timestamp(entry.mtime, 0)
                        .map(|dt| dt.format("%Y-%m-%d %H:%M").to_string())
                        .unwrap_or_else(|| entry.mtime.to_string()),
                    size_display: format_size(entry.size),
                    fullpath: full_path.clone(),
                    size: entry.size,
                    is_dir,
                });
            }

            if entry.mode & crate::serialization::S_IFDIR != 0 {
                stack.push((entry.id.clone(), full_path));
            }
        }
    }
}
