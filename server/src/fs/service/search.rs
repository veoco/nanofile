use std::sync::Arc;

use sea_orm::DatabaseConnection;

use crate::common::EMPTY_SHA1;
use crate::error::AppError;
use crate::repository::Repositories;
use crate::serialization::S_IFDIR;

/// A single file search result entry.
#[derive(serde::Serialize, Clone)]
pub struct FileSearchResult {
    pub repo_id: String,
    pub repo_name: String,
    pub name: String,
    pub oid: String,
    #[serde(alias = "mtime")]
    pub last_modified: i64,
    pub fullpath: String,
    pub size: i64,
    pub is_dir: bool,
}

pub struct SearchService {
    repos: Arc<Repositories>,
    db: Arc<DatabaseConnection>,
    indexer: Option<crate::indexer::TextIndexer>,
}

impl SearchService {
    pub fn new(
        repos: Arc<Repositories>,
        db: Arc<DatabaseConnection>,
        indexer: Option<crate::indexer::TextIndexer>,
    ) -> Self {
        Self { repos, db, indexer }
    }

    fn db(&self) -> &DatabaseConnection {
        self.db.as_ref()
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
        let db = self.db();

        let repo_ids = self.get_accessible_repo_ids(user_id, search_repo).await?;

        let mut seen = std::collections::HashSet::new();
        let mut all_results: Vec<FileSearchResult> = Vec::new();

        // Phase 1: Full-text search via Tantivy
        if let Some(indexer) = &self.indexer {
            let ft_results = match indexer.search(q, &repo_ids, 200, 0, search_filename_only) {
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
                if let Some(meta) = self
                    .resolve_file_metadata(found_repo_id, found_fullpath)
                    .await
                {
                    all_results.push(meta);
                }
            }
        }

        // Phase 2: Filename search
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
                db,
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

    /// Resolve file metadata to build a FileSearchResult.
    async fn resolve_file_metadata(
        &self,
        repo_id: &str,
        fullpath: &str,
    ) -> Option<FileSearchResult> {
        let db = self.db();
        let repo_record = self.repos.repo.find_by_id(repo_id).await.ok()??;
        let head_commit_id = repo_record.head_commit_id.as_ref()?;
        let head = self
            .repos
            .commit
            .find_by_repo_and_commit_id(repo_id, head_commit_id)
            .await
            .ok()??;
        let root_id = &head.root_id;
        if root_id == EMPTY_SHA1 {
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
            let path = format!("/{}", parent_segments.join("/"));
            Box::leak(path.into_boxed_str())
        };

        let parent_fs_id = if parent_path == "/" {
            root_id.clone()
        } else {
            crate::repo::resolve_fs_id(db, repo_id, root_id, parent_path)
                .await
                .ok()?
        };

        let dir_data = crate::repo::read_fs_dir_data(db, repo_id, &parent_fs_id)
            .await
            .ok()?;
        let entry = dir_data.dirents.iter().find(|d| d.name == *name)?;

        let is_dir = entry.mode & S_IFDIR != 0;
        Some(FileSearchResult {
            repo_id: repo_id.to_string(),
            repo_name: repo_record.name,
            name: entry.name.clone(),
            oid: entry.id.clone(),
            last_modified: entry.mtime,
            fullpath: fullpath.to_string(),
            size: entry.size,
            is_dir,
        })
    }
}

/// Recursively search the FS tree for files/directories whose name contains the keyword.
#[allow(clippy::too_many_arguments)]
async fn search_fs_tree(
    db: &DatabaseConnection,
    repo_id: &str,
    repo_name: &str,
    root_fs_id: &str,
    base_path: &str,
    keyword: &str,
    results: &mut Vec<FileSearchResult>,
    seen: &mut std::collections::HashSet<(String, String)>,
) {
    let keyword_lower = keyword.to_lowercase();
    let mut stack: Vec<(String, String)> = vec![(root_fs_id.to_string(), base_path.to_string())];

    while let Some((fs_id, path)) = stack.pop() {
        if fs_id == EMPTY_SHA1 {
            continue;
        }

        let dir_data = match crate::repo::read_fs_dir_data(db, repo_id, &fs_id).await {
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
                    // Already seen
                } else {
                    let is_dir = entry.mode & S_IFDIR != 0;
                    results.push(FileSearchResult {
                        repo_id: repo_id.to_string(),
                        repo_name: repo_name.to_string(),
                        name: entry.name.clone(),
                        oid: entry.id.clone(),
                        last_modified: entry.mtime,
                        fullpath: full_path.clone(),
                        size: entry.size,
                        is_dir,
                    });
                }
            }

            if entry.mode & S_IFDIR != 0 {
                stack.push((entry.id.clone(), full_path));
            }
        }
    }
}
