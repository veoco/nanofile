use std::sync::Arc;

use crate::domain::permission::{RepoScope, accessible_repo_ids};
use crate::repository::Repositories;
use base::error::AppError;
use infra::common::EMPTY_SHA1;
use infra::serialization::S_IFDIR;

/// Cap the total result set so a huge repo can't force a full-tree walk (and unbounded memory) on
/// every search. Phase 1 (content) and Phase 2 (filename) share this budget.
const MAX_SEARCH_RESULTS: usize = 200;

/// Largest page size the search endpoint accepts.
///
/// 100 matches the official clients: the iOS app requests `per_page=100` and the Android app 20,
/// and neither expects more than it asked for, so clamping cannot truncate a client-visible page.
pub const MAX_SEARCH_PER_PAGE: i32 = 100;

/// Largest page number the search endpoint accepts; only keeps the computed offset inside `i64`.
pub const MAX_SEARCH_PAGE: i32 = 10_000;

/// A single file search result entry.
#[derive(serde::Serialize, Clone)]
pub struct FileSearchResult {
    pub repo_id: String,
    pub repo_name: String,
    pub name: String,
    pub oid: String,
    pub last_modified: i64,
    /// iOS reads `mtime` (numeric), the desktop client reads `last_modified`; emit both so every
    /// client gets a valid timestamp.
    pub mtime: i64,
    pub fullpath: String,
    pub size: i64,
    pub is_dir: bool,
    /// URL pointing to the directory containing this file (or the directory itself).
    pub dir_url: String,
    /// HTML snippet with `<mark>` highlighting around the content match; empty for a filename-only
    /// match.
    pub content_highlight: String,
}

/// What to search for, and whose access it is searched under.
///
/// One implementation serves both the JSON endpoints and the Web UI page: the two used to be
/// separate copies of the same pipeline (filename walk, content index, dedup, ordering), which
/// drifted in query count, access scoping and result caps.
pub struct SearchRequest<'a> {
    pub q: &'a str,
    pub user_id: i32,
    /// A single library id, a seahub scope keyword (`all`/`mine`/`shared`/`group`/`public`), or
    /// `None` for every accessible library.
    pub search_repo: Option<&'a str>,
    /// Filename-only mode skips the content index; the tree walk always runs.
    pub filename_only: bool,
    /// The caller's library allow-list (a bound API key narrows it).
    pub repo_scope: &'a RepoScope,
}

/// One page of results, plus the totals the endpoints report.
pub struct SearchPage {
    pub results: Vec<FileSearchResult>,
    pub total: i32,
    pub has_more: bool,
}

pub struct SearchService {
    repos: Arc<Repositories>,
    indexer: Option<crate::indexer::TextIndexer>,
}

impl SearchService {
    pub fn new(repos: Arc<Repositories>, indexer: Option<crate::indexer::TextIndexer>) -> Self {
        Self { repos, indexer }
    }

    /// Search files across all accessible repos, returning one page of the collected hits.
    pub async fn search(
        &self,
        req: &SearchRequest<'_>,
        per_page: i32,
        page: i32,
    ) -> Result<SearchPage, AppError> {
        let hits = self.collect_hits(req).await?;

        // Clamp defensively here too: every caller must get bounded pagination, and the offset
        // below must not overflow.
        let per_page = per_page.clamp(1, MAX_SEARCH_PER_PAGE);
        let page = page.clamp(1, MAX_SEARCH_PAGE);
        let total = hits.len() as i32;
        // Both operands are clamped already, but compute in `i64` so a future change to the clamps
        // cannot silently overflow `i32` (a debug panic, a wrapped offset in release).
        let offset = (i64::from(page) - 1).saturating_mul(i64::from(per_page)) as usize;
        let page_end = offset.saturating_add(per_page as usize);

        let results = if offset < hits.len() {
            let end = page_end.min(hits.len());
            hits[offset..end].to_vec()
        } else {
            Vec::new()
        };
        let has_more = page_end < hits.len();

        Ok(SearchPage {
            results,
            total,
            has_more,
        })
    }

    /// Collect every hit the caller may see, up to the shared result budget.
    ///
    /// Callers that paginate themselves (the Web UI page, `/api/v2.1/search-file/`) use this
    /// directly instead of requesting one enormous page from [`SearchService::search`].
    pub async fn collect_hits(
        &self,
        req: &SearchRequest<'_>,
    ) -> Result<Vec<FileSearchResult>, AppError> {
        let q = req.q.trim();
        if q.is_empty() {
            return Ok(Vec::new());
        }

        let repo_ids = self
            .get_accessible_repo_ids(req.user_id, req.search_repo, req.repo_scope)
            .await?;
        // `repo_ids` is the caller's access-control allow-list: an empty list must never be handed
        // to the indexer as "no filter" (that leaked every repo's filenames and content snippets).
        // The indexer also fails closed on an empty list; this early return keeps it explicit here.
        let has_accessible_repos = !repo_ids.is_empty();

        let mut seen = std::collections::HashSet::new();
        let mut all_results: Vec<FileSearchResult> = Vec::new();

        // Phase 1: full-text (content) search via Tantivy, in full-text mode only. Filename-only
        // mode skips it because the FS tree walk below is the authoritative filename matcher and
        // covers binary files the index never sees.
        if has_accessible_repos
            && !req.filename_only
            && let Some(indexer) = &self.indexer
        {
            match indexer
                .search(q, &repo_ids, MAX_SEARCH_RESULTS, 0, false)
                .await
            {
                Ok(ft_results) => {
                    // Unique hits grouped by repo, so the repo record + head commit are resolved
                    // once per repo and all hit paths in a shared batched walk.
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

        // Phase 2: filename search via FS tree walk — always run, since it is the only complete
        // filename matcher (covers binary/non-indexed files and does true substring matching).
        // Repo records and head commits are batch-loaded up front (2 queries total) instead of a
        // `find_by_id` + `find_by_repo_and_commit_id` pair per repo (2N round-trips).
        let repos_map: std::collections::HashMap<String, _> = self
            .repos
            .repo
            .find_by_ids(&repo_ids)
            .await
            .unwrap_or_default()
            .into_iter()
            .map(|r| (r.id.clone(), r))
            .collect();

        let head_commit_ids: Vec<String> = repos_map
            .values()
            .filter_map(|r| r.head_commit_id.clone())
            .collect();
        let commits_map: std::collections::HashMap<String, _> = self
            .repos
            .commit
            .find_by_commit_ids(&head_commit_ids)
            .await
            .unwrap_or_default()
            .into_iter()
            .map(|c| (c.commit_id.clone(), c))
            .collect();

        // Phase 2 only tops up the budget left over from Phase 1; once the cap is reached the walk
        // stops, so a huge repo cannot force a full traversal on every search.
        let phase2_budget = MAX_SEARCH_RESULTS.saturating_sub(all_results.len());
        if phase2_budget > 0 {
            for repo_id in &repo_ids {
                let Some(repo_record) = repos_map.get(repo_id) else {
                    continue;
                };
                let Some(head_commit_id) = &repo_record.head_commit_id else {
                    continue;
                };
                let Some(head) = commits_map.get(head_commit_id) else {
                    continue;
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
                    phase2_budget,
                )
                .await;
            }
        }

        all_results.sort_by(|a, b| b.is_dir.cmp(&a.is_dir).then_with(|| a.name.cmp(&b.name)));
        Ok(all_results)
    }

    async fn get_accessible_repo_ids(
        &self,
        user_id: i32,
        repo_id_filter: Option<&str>,
        repo_scope: &RepoScope,
    ) -> Result<Vec<String>, AppError> {
        let mut ids: Vec<String> = accessible_repo_ids(&self.repos, user_id, repo_scope)
            .await?
            .into_iter()
            .collect();
        ids.sort();

        if let Some(filter) = repo_id_filter {
            // seahub scope keywords ("all"/"mine"/"shared"/"group"/"public") mean "search across
            // all accessible repos", not a specific repo. Seadroid sends search_repo="all" for
            // global search; treating it as a repo id would filter out every repo.
            //
            // seahub lowercases `search_repo` before comparing (`api2/views.py`), so the scope
            // match must be case-insensitive for a client that sends e.g. "ALL".
            let scope = filter.trim().to_ascii_lowercase();
            let is_scope = matches!(
                scope.as_str(),
                "all" | "mine" | "shared" | "group" | "public"
            );
            if !is_scope {
                // A filter naming a repo the caller cannot access leaves the list empty, which
                // means "no access" — NOT "no filter". It must never widen the search (see
                // `collect_hits`).
                ids.retain(|id| id == filter);
            }
        }

        Ok(ids)
    }

    /// Resolve file metadata for many fullpaths of a single repo in a shared batched walk, returning
    /// `None` for paths that no longer resolve. `repo_name`/`root_id` are resolved once per repo by
    /// the caller (see [`SearchService::collect_hits`]).
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

        // Resolve every parent path in one batched walk (a "/" parent resolves to root_id itself).
        let targets: Vec<(String, String)> = metas
            .iter()
            .map(|(_, parent_path)| (root_id.to_string(), parent_path.clone()))
            .collect();
        let Ok(resolved) =
            crate::fs::core::resolve_fs_ids_batch(&self.repos, repo_id, &targets).await
        else {
            return vec![None; fullpaths.len()];
        };

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
///
/// Stops early once `results.len()` reaches `max_results`, so a huge repo doesn't force a full
/// traversal when the budget is already filled.
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
    max_results: usize,
) {
    let keyword_lower = keyword.to_lowercase();
    // Level frontier: each level reads all its directories with one batched `IN` query
    // (O(#dirs) → O(depth)).
    let mut frontier: Vec<(String, String)> = vec![(root_fs_id.to_string(), base_path.to_string())];
    let max_depth = crate::fs::core::traversal::max_tree_depth();
    let mut depth = 0usize;

    while !frontier.is_empty() && results.len() < max_results {
        // Cycle guard: this helper returns no error, so cap the depth and
        // stop rather than expanding forever.
        depth += 1;
        if depth > max_depth {
            tracing::warn!(
                repo_id,
                max_depth,
                "file-tree search hit the traversal depth cap; truncating"
            );
            break;
        }
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
                // The reserved `_Internal` tree is not part of what a search may reveal.
                if crate::service::fs::dir::is_reserved_internal(&entry.name) {
                    continue;
                }

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
                        if results.len() >= max_results {
                            return;
                        }
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
