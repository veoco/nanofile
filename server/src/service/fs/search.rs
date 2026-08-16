use std::sync::Arc;

use crate::repository::Repositories;
use base::error::AppError;
use infra::common::EMPTY_SHA1;
use infra::serialization::S_IFDIR;

/// A single file search result entry.
#[derive(serde::Serialize, Clone)]
pub struct FileSearchResult {
    pub repo_id: String,
    pub repo_name: String,
    pub name: String,
    pub oid: String,
    pub last_modified: i64,
    /// iOS reads `mtime` (numeric), the desktop client reads `last_modified`;
    /// emit both so every client gets a valid timestamp.
    pub mtime: i64,
    pub fullpath: String,
    pub size: i64,
    pub is_dir: bool,
    /// URL pointing to the directory containing this file (or the directory itself).
    pub dir_url: String,
    /// HTML snippet with `<mark>` highlighting around the content match.
    /// Empty when the result came from a filename-only match.
    pub content_highlight: String,
}

pub struct SearchService {
    repos: Arc<Repositories>,
    indexer: Option<crate::indexer::TextIndexer>,
}

impl SearchService {
    pub fn new(repos: Arc<Repositories>, indexer: Option<crate::indexer::TextIndexer>) -> Self {
        Self { repos, indexer }
    }

    /// Search files across all accessible repos.
    pub async fn search(
        &self,
        q: &str,
        user_id: i32,
        per_page: i32,
        page: i32,
        search_repo: Option<&str>,
        search_filename_only: bool,
    ) -> Result<(Vec<serde_json::Value>, i32, bool), AppError> {
        if q.is_empty() {
            return Ok((Vec::new(), 0, false));
        }

        let per_page = per_page.max(1);
        let page = page.max(1);
        let repo_ids = self.get_accessible_repo_ids(user_id, search_repo).await?;

        let mut seen = std::collections::HashSet::new();
        let mut all_results: Vec<FileSearchResult> = Vec::new();

        // Phase 1: Full-text (content) search via Tantivy — only in full-text
        // mode. Filename-only mode skips it because the FS tree walk below is
        // the authoritative filename matcher and covers binary files the index
        // never sees.
        if !search_filename_only && let Some(indexer) = &self.indexer {
            match indexer.search(q, &repo_ids, 200, 0, false).await {
                Ok(ft_results) => {
                    // Collect unique hits, then group by repo so the repo
                    // record + head commit are resolved once per repo and all
                    // hit paths are resolved in a shared batched walk.
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
                        let repo_record = match self.repos.repo.find_by_id(&found_repo_id).await {
                            Ok(Some(r)) => r,
                            _ => continue,
                        };
                        let head_commit_id = match &repo_record.head_commit_id {
                            Some(id) => id.clone(),
                            None => continue,
                        };
                        let head = match self
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
                        let metas = self
                            .resolve_file_metadata_batch(
                                &found_repo_id,
                                &repo_record.name,
                                &head.root_id,
                                &fullpaths,
                                &highlights,
                            )
                            .await;
                        all_results.extend(metas.into_iter().flatten());
                    }
                }
                Err(e) => {
                    tracing::warn!("Tantivy search failed: {e}");
                }
            }
        }

        // Phase 2: Filename search via FS tree walk — always run, since it is
        // the only complete filename matcher (covers binary/non-indexed files
        // and performs true substring matching).
        for repo_id in &repo_ids {
            let repo_record = match self.repos.repo.find_by_id(repo_id).await {
                Ok(Some(r)) => r,
                _ => continue,
            };

            let head_commit_id = match &repo_record.head_commit_id {
                Some(id) => id.clone(),
                None => continue,
            };

            let head = match self
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
                &self.repos,
                repo_id,
                &repo_record.name,
                &head.root_id,
                "",
                q,
                &mut all_results,
                &mut seen,
            )
            .await;
        }

        // Sort: directories first, then by name
        all_results.sort_by(|a, b| b.is_dir.cmp(&a.is_dir).then_with(|| a.name.cmp(&b.name)));

        let total = all_results.len() as i32;
        let offset = ((page - 1) * per_page) as usize;

        let results: Vec<serde_json::Value> = if offset < all_results.len() {
            let end = (offset + per_page as usize).min(all_results.len());
            all_results[offset..end]
                .iter()
                .map(|r| serde_json::json!(r))
                .collect()
        } else {
            Vec::new()
        };

        let has_more = (offset + per_page as usize) < all_results.len();

        Ok((results, total, has_more))
    }

    /// Get repo IDs accessible to the user.
    async fn get_accessible_repo_ids(
        &self,
        user_id: i32,
        repo_id_filter: Option<&str>,
    ) -> Result<Vec<String>, AppError> {
        let member_repos = self.repos.member.find_by_user_id(user_id).await?;
        let owned_repos = self.repos.repo.find_by_owner_id(user_id).await?;

        let mut ids: Vec<String> = member_repos
            .into_iter()
            .map(|m| m.repo_id)
            .chain(owned_repos.into_iter().map(|r| r.id))
            .collect();

        ids.sort();
        ids.dedup();

        if let Some(filter) = repo_id_filter {
            ids.retain(|id| id == filter);
        }

        Ok(ids)
    }

    /// Resolve file metadata for many fullpaths of a single repo in a shared
    /// batched walk, returning `None` for paths that no longer resolve.
    ///
    /// `repo_name`/`root_id` are resolved once per repo by the caller (see
    /// [`SearchService::search`]).
    async fn resolve_file_metadata_batch(
        &self,
        repo_id: &str,
        repo_name: &str,
        root_id: &str,
        fullpaths: &[String],
        highlights: &[String],
    ) -> Vec<Option<FileSearchResult>> {
        if root_id == EMPTY_SHA1 || fullpaths.is_empty() {
            return vec![None; fullpaths.len()];
        }

        // Pre-compute (name, parent_path) for each fullpath.
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

        // Resolve every parent path in one batched walk (a "/" parent resolves
        // to root_id itself).
        let targets: Vec<(String, String)> = metas
            .iter()
            .map(|(_, parent_path)| (root_id.to_string(), parent_path.clone()))
            .collect();
        let Ok(resolved) =
            crate::fs::core::resolve_fs_ids_batch(&self.repos, repo_id, &targets).await
        else {
            return vec![None; fullpaths.len()];
        };

        // Batch-read all distinct parent directories.
        let mut parent_ids: Vec<String> = resolved.iter().filter_map(|r| r.clone()).collect();
        parent_ids.sort();
        parent_ids.dedup();
        let Ok(dir_map) =
            crate::fs::core::read_fs_dir_data_batch(&self.repos, repo_id, &parent_ids).await
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

            let is_dir = entry.mode & S_IFDIR != 0;
            let dir_url = if is_dir {
                format!("/libraries/{}/files{}", repo_id, fullpath)
            } else {
                let parent = fullpath
                    .rsplit_once('/')
                    .map(|(parent, _)| parent)
                    .unwrap_or("/");
                format!("/libraries/{}/files{}", repo_id, parent)
            };

            results.push(Some(FileSearchResult {
                repo_id: repo_id.to_string(),
                repo_name: repo_name.to_string(),
                name: entry.name.clone(),
                oid: entry.id.clone(),
                last_modified: entry.mtime,
                mtime: entry.mtime,
                fullpath: fullpath.clone(),
                size: entry.size,
                is_dir,
                dir_url,
                content_highlight: highlights.get(i).cloned().unwrap_or_default(),
            }));
        }
        results
    }
}

/// Recursively search the FS tree for files/directories whose name contains the keyword.
#[allow(clippy::too_many_arguments)]
async fn search_fs_tree(
    repos: &Repositories,
    repo_id: &str,
    repo_name: &str,
    root_fs_id: &str,
    base_path: &str,
    keyword: &str,
    results: &mut Vec<FileSearchResult>,
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
                    if seen.insert(key) {
                        let is_dir = entry.mode & S_IFDIR != 0;
                        let dir_url = if is_dir {
                            format!("/libraries/{}/files{}", repo_id, full_path)
                        } else {
                            let parent = full_path
                                .rsplit_once('/')
                                .map(|(parent, _)| parent)
                                .unwrap_or("/");
                            format!("/libraries/{}/files{}", repo_id, parent)
                        };
                        results.push(FileSearchResult {
                            repo_id: repo_id.to_string(),
                            repo_name: repo_name.to_string(),
                            name: entry.name.clone(),
                            oid: entry.id.clone(),
                            last_modified: entry.mtime,
                            mtime: entry.mtime,
                            fullpath: full_path.clone(),
                            size: entry.size,
                            is_dir,
                            dir_url,
                            content_highlight: String::new(),
                        });
                    }
                }

                if entry.mode & S_IFDIR != 0 {
                    next.push((entry.id.clone(), full_path));
                }
            }
        }

        frontier = next;
    }
}
