use axum::{
    Json,
    extract::{Query, State},
};
use sea_orm::{ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

use crate::AppState;
use crate::auth::middleware::AuthUser;
use crate::entity::{commit, repo, repo_member};
use crate::error::AppError;
use crate::serialization::S_IFDIR;

/// The well-known sentinel for empty directories in seafile's protocol.
const EMPTY_SHA1: &str = "0000000000000000000000000000000000000000";

#[derive(Deserialize)]
pub struct SearchQuery {
    pub q: Option<String>,
    pub per_page: Option<i32>,
    pub page: Option<i32>,
    pub search_repo: Option<String>,
    /// When true (or when indexer is unavailable), search filenames only.
    /// When false, also search file content via the full-text index.
    /// Defaults to true for backward compatibility.
    pub search_filename_only: Option<bool>,
}

#[derive(Serialize)]
pub struct SearchResponse {
    pub results: Vec<serde_json::Value>,
    pub total: i32,
    pub has_more: bool,
}

/// A single file search result entry, matching the format expected by
/// seafile-client (Qt) and Seahub Web UI.
#[derive(Serialize)]
struct FileSearchResult {
    repo_id: String,
    repo_name: String,
    name: String,
    oid: String,
    #[serde(alias = "mtime")]
    last_modified: i64,
    fullpath: String,
    size: i64,
    is_dir: bool,
}

/// GET /api2/search/?q=&per_page=&page=&search_repo=
///
/// Searches file/directory names across all repos accessible to the user.
/// Uses case-insensitive substring matching against file/directory names,
/// recursively traversing the FS object tree from each repo's head commit.
///
/// This mirrors seafile-server's `search_files_recursive` (common/fs-mgr.c)
/// which uses `strcasestr` for name matching.
pub async fn search(
    auth: AuthUser,
    State(state): State<Arc<AppState>>,
    Query(query): Query<SearchQuery>,
) -> Result<Json<SearchResponse>, AppError> {
    let q = query.q.unwrap_or_default().trim().to_string();
    if q.is_empty() {
        return Ok(Json(SearchResponse {
            results: Vec::new(),
            total: 0,
            has_more: false,
        }));
    }

    let per_page = query.per_page.unwrap_or(10).max(1);
    let page = query.page.unwrap_or(1).max(1);

    // search_filename_only defaults to true for backward compatibility with
    // the seafile-client (Qt) which only does filename search.
    let search_filename_only = query.search_filename_only.unwrap_or(true);

    let db = state.db.as_ref();

    // Get repo IDs accessible to this user (membership + ownership)
    let repo_ids = get_accessible_repo_ids(db, auth.user_id, query.search_repo.as_deref()).await?;

    // Collect all matching results across all accessible repos.
    // Use a HashSet of (repo_id, fullpath) to deduplicate.
    let mut seen = std::collections::HashSet::new();
    let mut all_results: Vec<FileSearchResult> = Vec::new();

    // Phase 1: Full-text search via Tantivy (when available and not filename-only).
    if !search_filename_only && let Some(indexer) = &state.indexer {
        // Search up to 200 results from the index to cover pagination needs.
        let ft_results = indexer.search(&q, &repo_ids, 200, 0).unwrap_or_default();
        for (found_repo_id, found_fullpath) in &ft_results {
            if !seen.insert((found_repo_id.clone(), found_fullpath.clone())) {
                continue; // Already seen
            }
            // Look up metadata from DB so we can fill in oid, size, mtime, is_dir.
            if let Some(meta) = resolve_file_metadata(db, found_repo_id, found_fullpath).await {
                all_results.push(meta);
            }
        }
    }

    // Phase 2: Filename search (always, for non-empty keyword).
    for repo_id in &repo_ids {
        let repo_record = match repo::Entity::find_by_id(repo_id).one(db).await {
            Ok(Some(r)) => r,
            _ => continue,
        };

        let head_commit_id = match &repo_record.head_commit_id {
            Some(id) => id.clone(),
            None => continue, // Empty repo, nothing to search
        };

        let head = match commit::Entity::find()
            .filter(commit::Column::RepoId.eq(repo_id))
            .filter(commit::Column::CommitId.eq(&head_commit_id))
            .one(db)
            .await
        {
            Ok(Some(h)) => h,
            _ => continue,
        };

        // Skip empty repos (EMPTY_SHA1 sentinel root)
        if head.root_id == EMPTY_SHA1 {
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

    // Sort results: directories first, then by name
    all_results.sort_by(|a, b| b.is_dir.cmp(&a.is_dir).then_with(|| a.name.cmp(&b.name)));

    // Paginate
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

    Ok(Json(SearchResponse {
        results,
        total,
        has_more,
    }))
}

/// Get repo IDs accessible to the user.
///
/// A user can access repos where they are:
/// - A member in the `repo_member` table
/// - The owner in the `repo` table
///
/// If `repo_id_filter` is provided, only check access to that specific repo.
async fn get_accessible_repo_ids(
    db: &DatabaseConnection,
    user_id: i32,
    repo_id_filter: Option<&str>,
) -> Result<Vec<String>, AppError> {
    // Repos where the user is a member
    let member_repos = repo_member::Entity::find()
        .filter(repo_member::Column::UserId.eq(user_id))
        .all(db)
        .await?;

    // Also repos owned by the user (ownership may exist without membership)
    let owned_repos = repo::Entity::find()
        .filter(repo::Column::OwnerId.eq(user_id))
        .all(db)
        .await?;

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

/// Recursively search the FS tree for files/directories whose name
/// contains the keyword (case-insensitive substring match).
///
/// Uses an iterative stack to avoid deep recursion, matching the behavior
/// of seafile-server's `search_files_recursive` in common/fs-mgr.c.
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

        let dir_data = match crate::storage::read_fs_dir_data(db, repo_id, &fs_id).await {
            Ok(data) => data,
            Err(_) => continue, // Skip unreadable/corrupt objects
        };

        for entry in &dir_data.dirents {
            let full_path = if path.is_empty() {
                format!("/{}", entry.name)
            } else if path.starts_with('/') {
                format!("{}/{}", path, entry.name)
            } else {
                format!("/{}/{}", path, entry.name)
            };

            // Case-insensitive substring match (seafile-server uses strcasestr)
            if entry.name.to_lowercase().contains(&keyword_lower) {
                let key = (repo_id.to_string(), full_path.clone());
                if !seen.insert(key) {
                    // Already seen (e.g. from full-text index) — skip
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

            // Recurse into subdirectories
            if entry.mode & S_IFDIR != 0 {
                stack.push((entry.id.clone(), full_path));
            }
        }
    }
}

/// Resolve file metadata from the DB to build a FileSearchResult for a
/// (repo_id, fullpath) pair discovered by full-text search.
async fn resolve_file_metadata(
    db: &DatabaseConnection,
    repo_id: &str,
    fullpath: &str,
) -> Option<FileSearchResult> {
    let repo_record = repo::Entity::find_by_id(repo_id).one(db).await.ok()??;
    let head_commit_id = repo_record.head_commit_id.as_ref()?;
    let head = commit::Entity::find()
        .filter(commit::Column::RepoId.eq(repo_id))
        .filter(commit::Column::CommitId.eq(head_commit_id))
        .one(db)
        .await
        .ok()??;
    let root_id = &head.root_id;
    if root_id == EMPTY_SHA1 {
        return None;
    }

    // Resolve the path to find the fs_id, then derive metadata.
    // seafile's FS tree: split path, traverse.
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
        // Leak for lifetime — this is fine for a local function.
        Box::leak(path.into_boxed_str())
    };

    // Get the parent directory
    let parent_fs_id = if parent_path == "/" {
        root_id.clone()
    } else {
        crate::storage::resolve_fs_id(db, repo_id, root_id, parent_path)
            .await
            .ok()?
    };

    let dir_data = crate::storage::read_fs_dir_data(db, repo_id, &parent_fs_id)
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
