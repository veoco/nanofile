/// Web UI search handler.
use askama::Template;
use axum::{
    extract::{Query, State},
    response::Html,
};
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
    pub left_panel_repos: Vec<crate::repo::LeftPanelRepo>,
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
    pub last_modified_readable: String,
    #[serde(skip)]
    pub size_display: String,
    pub fullpath: String,
    pub size: i64,
    pub is_dir: bool,
    /// URL for the directory containing this file (or the directory itself).
    #[serde(skip)]
    pub dir_url: String,
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
        let repo_ids = get_accessible_repo_ids(&state.repos, user.user_id, None).await?;
        let mut seen = std::collections::HashSet::new();
        let mut all_results: Vec<SearchResultItem> = Vec::new();

        // Phase 1: Full-text search via Tantivy (when available).
        // This runs for both filename-only and content searches, avoiding
        // the expensive FS tree walk for filename matching.
        if let Some(indexer) = &state.indexer {
            let ft_results = match indexer.search(&q, &repo_ids, 200, 0, search_filename_only) {
                Ok(r) => r,
                Err(e) => {
                    tracing::warn!("Tantivy search failed: {e}");
                    Vec::new()
                }
            };
            for (found_repo_id, found_fullpath) in &ft_results {
                if !seen.insert((found_repo_id.clone(), found_fullpath.clone())) {
                    continue;
                }
                if let Some(item) =
                    resolve_file_metadata(&state.repos, found_repo_id, found_fullpath).await
                {
                    all_results.push(item);
                }
            }
        }

        // Phase 2: Filename search via FS tree walk (fallback for repos
        // not covered by the index, or when the indexer is disabled).
        for repo_id in &repo_ids {
            let repo_record = match state.repos.repo.find_by_id(repo_id).await {
                Ok(Some(r)) => r,
                _ => continue,
            };

            let head_commit_id = match &repo_record.head_commit_id {
                Some(id) => id.clone(),
                None => continue,
            };

            let head = match state
                .repos
                .commit
                .find_by_repo_and_commit_id(repo_id, &head_commit_id)
                .await
            {
                Ok(Some(h)) => h,
                _ => continue,
            };

            if head.root_id == "0000000000000000000000000000000000000000" {
                continue;
            }

            search_fs_tree(
                &state.repos,
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

    let left_panel_repos = crate::repo::load_left_panel_repos(&state.repos, user.user_id).await?;
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
        left_panel_repos,
        current_repo_id: None,
    };
    let html = tpl
        .render()
        .map_err(|e| AppError::internal(e.to_string()))?;
    Ok(Html(html))
}

/// Resolve file metadata from DB to build a SearchResultItem for a
/// (repo_id, fullpath) pair discovered by full-text search.
async fn resolve_file_metadata(
    repos: &crate::repository::Repositories,
    repo_id: &str,
    fullpath: &str,
) -> Option<SearchResultItem> {
    let repo_record = repos.repo.find_by_id(repo_id).await.ok()??;
    let head_commit_id = repo_record.head_commit_id.as_ref()?;
    let head = repos
        .commit
        .find_by_repo_and_commit_id(repo_id, head_commit_id)
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
        crate::fs::core::resolve_fs_id(repos, repo_id, root_id, parent_path)
            .await
            .ok()?
    };

    let dir_data = crate::fs::core::read_fs_dir_data(repos, repo_id, &parent_fs_id)
        .await
        .ok()?;
    let entry = dir_data.dirents.iter().find(|d| d.name == *name)?;

    let is_dir = entry.mode & crate::serialization::S_IFDIR != 0;

    // Compute the directory URL: for files, point to the parent directory;
    // for directories, point to the directory itself.
    let dir_url = if is_dir {
        format!("/libraries/{}/files{}", repo_id, fullpath)
    } else {
        let parent = fullpath
            .rsplit_once('/')
            .map(|(parent, _)| parent)
            .unwrap_or("/");
        format!("/libraries/{}/files{}", repo_id, parent)
    };

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
        dir_url,
    })
}

async fn get_accessible_repo_ids(
    repos: &crate::repository::Repositories,
    user_id: i32,
    _repo_id_filter: Option<&str>,
) -> Result<Vec<String>, AppError> {
    let member_repos = repos.member.find_by_user_id(user_id).await?;

    let owned_repos = repos.repo.find_by_owner_id(user_id).await?;

    let mut ids: Vec<String> = member_repos
        .into_iter()
        .map(|m| m.repo_id)
        .chain(owned_repos.into_iter().map(|r| r.id))
        .collect();

    ids.sort();
    ids.dedup();

    Ok(ids)
}

#[allow(clippy::too_many_arguments)]
async fn search_fs_tree(
    repos: &crate::repository::Repositories,
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

        let dir_data = match crate::fs::core::read_fs_dir_data(repos, repo_id, &fs_id).await {
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
                let dir_url = if is_dir {
                    format!("/libraries/{}/files{}", repo_id, full_path)
                } else {
                    let parent = full_path
                        .rsplit_once('/')
                        .map(|(parent, _)| parent)
                        .unwrap_or("/");
                    format!("/libraries/{}/files{}", repo_id, parent)
                };
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
                    dir_url,
                });
            }

            if entry.mode & crate::serialization::S_IFDIR != 0 {
                stack.push((entry.id.clone(), full_path));
            }
        }
    }
}
