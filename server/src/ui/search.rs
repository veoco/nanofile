/// Web UI search handler.
use askama::Template;
use axum::{
    extract::{Query, State},
    response::Html,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

use crate::AppState;
use crate::i18n::I18n;
use crate::ui::files::format_size;
use base::common::EMPTY_SHA1;
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
    /// HTML snippet with `<mark>` highlighting around the content match.
    /// Empty when the result came from a filename-only match.
    pub content_highlight: String,
}

#[derive(Deserialize)]
pub struct SearchQuery {
    pub q: Option<String>,
    /// When absent or false, search both filenames and file content (default).
    /// When true, search filenames only.
    pub search_filename_only: Option<bool>,
}

/// GET /search?q=xxx — search page (Web UI).
pub async fn search_page(
    user: WebUser,
    State(state): State<Arc<AppState>>,
    Query(query): Query<SearchQuery>,
) -> Result<Html<String>, AppError> {
    let q = query.q.unwrap_or_default();
    let search_filename_only = query.search_filename_only.unwrap_or(false);
    let per_page: i32 = 20;
    let page: i32 = 1;

    let (results, total, has_more) = if q.trim().is_empty() {
        (Vec::new(), 0, false)
    } else {
        let repo_ids = get_accessible_repo_ids(&state.repos, user.user_id, None).await?;
        let mut seen = std::collections::HashSet::new();
        let mut all_results: Vec<SearchResultItem> = Vec::new();

        // Phase 1: Full-text (content) search via Tantivy — only in full-text
        // mode. Filename-only mode skips it because the FS tree walk below is
        // the authoritative filename matcher and covers binary files the index
        // never sees.
        if !search_filename_only && let Some(indexer) = &state.indexer {
            let ft_results = match indexer.search(&q, &repo_ids, 200, 0, false).await {
                Ok(r) => r,
                Err(e) => {
                    tracing::warn!("Tantivy search failed: {e}");
                    Vec::new()
                }
            };
            // Collect unique hits, then group by repo so the repo record +
            // head commit are resolved once per repo and all hit paths are
            // resolved in a shared batched walk.
            let mut unique_hits: Vec<crate::indexer::IndexHit> = Vec::new();
            for hit in &ft_results {
                if seen.insert((hit.repo_id.clone(), hit.fullpath.clone())) {
                    unique_hits.push(hit.clone());
                }
            }

            let mut by_repo: std::collections::HashMap<String, Vec<usize>> =
                std::collections::HashMap::new();
            for (idx, hit) in unique_hits.iter().enumerate() {
                by_repo.entry(hit.repo_id.clone()).or_default().push(idx);
            }

            for (found_repo_id, indices) in by_repo {
                let repo_record = match state.repos.repo.find_by_id(&found_repo_id).await {
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
                    .find_by_repo_and_commit_id(&found_repo_id, &head_commit_id)
                    .await
                {
                    Ok(Some(h)) => h,
                    _ => continue,
                };
                if head.root_id == EMPTY_SHA1 {
                    continue;
                }

                let fullpaths: Vec<String> = indices
                    .iter()
                    .map(|&i| unique_hits[i].fullpath.clone())
                    .collect();
                let highlights: Vec<String> = indices
                    .iter()
                    .map(|&i| unique_hits[i].content_highlight.clone())
                    .collect();
                let items = resolve_file_metadata_batch(
                    &state.repos,
                    &found_repo_id,
                    &repo_record.name,
                    &head.root_id,
                    &fullpaths,
                    &highlights,
                )
                .await;
                all_results.extend(items.into_iter().flatten());
            }
        }

        // Phase 2: Filename search via FS tree walk — always run, since it is
        // the only complete filename matcher (covers binary/non-indexed files
        // and performs true substring matching).
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

            if head.root_id == EMPTY_SHA1 {
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
        current_repo_id: None,
    };
    let html = tpl
        .render()
        .map_err(|e| AppError::internal(e.to_string()))?;
    Ok(Html(html))
}

/// Resolve file metadata for many fullpaths of a single repo in a shared
/// batched walk, returning `None` for paths that no longer resolve.
///
/// `repo_name`/`root_id` are resolved once per repo by the caller so the
/// repo record + head commit are not re-fetched for every hit.
async fn resolve_file_metadata_batch(
    repos: &crate::repository::Repositories,
    repo_id: &str,
    repo_name: &str,
    root_id: &str,
    fullpaths: &[String],
    highlights: &[String],
) -> Vec<Option<SearchResultItem>> {
    if root_id == EMPTY_SHA1 || fullpaths.is_empty() {
        return vec![None; fullpaths.len()];
    }

    let metas: Vec<(String, String)> = fullpaths
        .iter()
        .map(|fullpath| {
            let segments: Vec<&str> = fullpath
                .trim_start_matches('/')
                .split('/')
                .filter(|s| !s.is_empty())
                .collect();
            let name = segments.last().map(|s| s.to_string()).unwrap_or_default();
            let parent_path = if segments.len() <= 1 {
                String::from("/")
            } else {
                format!("/{}", segments[..segments.len() - 1].join("/"))
            };
            (name, parent_path)
        })
        .collect();

    let targets: Vec<(String, String)> = metas
        .iter()
        .map(|(_, parent_path)| (root_id.to_string(), parent_path.clone()))
        .collect();
    let Ok(resolved) = crate::fs::core::resolve_fs_ids_batch(repos, repo_id, &targets).await else {
        return vec![None; fullpaths.len()];
    };

    let mut parent_ids: Vec<String> = resolved.iter().filter_map(|r| r.clone()).collect();
    parent_ids.sort();
    parent_ids.dedup();
    let Ok(dir_map) = crate::fs::core::read_fs_dir_data_batch(repos, repo_id, &parent_ids).await
    else {
        return vec![None; fullpaths.len()];
    };

    let mut results = Vec::with_capacity(fullpaths.len());
    for (i, fullpath) in fullpaths.iter().enumerate() {
        let (name, _) = &metas[i];
        let Some(parent_fs_id) = &resolved[i] else {
            results.push(None);
            continue;
        };
        let Some(dir_data) = dir_map.get(parent_fs_id) else {
            results.push(None);
            continue;
        };
        let Some(entry) = dir_data.dirents.iter().find(|d| d.name.as_str() == name) else {
            results.push(None);
            continue;
        };

        let is_dir = entry.mode & infra::serialization::S_IFDIR != 0;
        let dir_url = if is_dir {
            format!("/libraries/{}/files{}", repo_id, fullpath)
        } else {
            let parent = fullpath
                .rsplit_once('/')
                .map(|(parent, _)| parent)
                .unwrap_or("/");
            format!("/libraries/{}/files{}", repo_id, parent)
        };

        results.push(Some(SearchResultItem {
            repo_id: repo_id.to_string(),
            repo_name: repo_name.to_string(),
            name: entry.name.clone(),
            oid: entry.id.clone(),
            last_modified: entry.mtime,
            last_modified_readable: chrono::DateTime::from_timestamp(entry.mtime, 0)
                .map(|dt| dt.format("%Y-%m-%d %H:%M").to_string())
                .unwrap_or_else(|| entry.mtime.to_string()),
            size_display: format_size(entry.size),
            fullpath: fullpath.clone(),
            size: entry.size,
            is_dir,
            dir_url,
            content_highlight: highlights.get(i).cloned().unwrap_or_default(),
        }));
    }
    results
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
    // Level frontier: each level reads all its directories with one batched
    // `IN` query (O(#dirs) → O(depth)).
    let mut frontier: Vec<(String, String)> = vec![(root_fs_id.to_string(), base_path.to_string())];

    while !frontier.is_empty() {
        let ids: Vec<String> = frontier
            .iter()
            .map(|(fs_id, _)| fs_id.clone())
            .filter(|id| id != EMPTY_SHA1)
            .collect();
        let dir_map = match crate::fs::core::read_fs_dir_data_batch(repos, repo_id, &ids).await {
            Ok(m) => m,
            Err(_) => break,
        };
        let mut next: Vec<(String, String)> = Vec::new();

        for (fs_id, path) in &frontier {
            // Missing/EMPTY dirs are absent from the batch map → skip.
            let Some(dir_data) = dir_map.get(fs_id) else {
                continue;
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
                    let is_dir = entry.mode & infra::serialization::S_IFDIR != 0;
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
                        content_highlight: String::new(),
                    });
                }

                if entry.mode & infra::serialization::S_IFDIR != 0 {
                    next.push((entry.id.clone(), full_path));
                }
            }
        }

        frontier = next;
    }
}
