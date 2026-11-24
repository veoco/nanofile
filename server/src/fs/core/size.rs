use crate::repository::Repositories;
use base::error::AppError;
use infra::common::EMPTY_SHA1;
use infra::common::util::basename;
use infra::serialization::S_IFDIR;

use crate::fs::core::{read_fs_dir_data, read_fs_dir_data_batch, resolve_fs_id};

/// Compute the total size of all files in the repo's HEAD tree. Returns 0 when
/// there is no commit or the tree is empty.
pub async fn compute_repo_size(repos: &Repositories, repo_id: &str) -> Result<i64, AppError> {
    let head = crate::fs::core::repo_head(repos, repo_id).await?;
    if head.head_commit_id.is_none() {
        return Ok(0);
    }
    compute_tree_size(repos, repo_id, &head.root_id).await
}

/// Walk the directory tree rooted at `root_fs_id` and sum file sizes.
pub async fn compute_tree_size(
    repos: &Repositories,
    repo_id: &str,
    root_fs_id: &str,
) -> Result<i64, AppError> {
    Ok(walk_tree(repos, repo_id, root_fs_id, "", false)
        .await?
        .total)
}

/// Walk a directory subtree, returning its total size and every file path below
/// it (repo-absolute, joined under `path_prefix`).
///
/// A delete of a directory needs both the quota delta and the list of index
/// documents to remove; one walk serves both so the tree is read once.
pub async fn compute_tree_size_and_files(
    repos: &Repositories,
    repo_id: &str,
    root_fs_id: &str,
    path_prefix: &str,
) -> Result<(i64, Vec<String>), AppError> {
    let walk = walk_tree(repos, repo_id, root_fs_id, path_prefix, true).await?;
    Ok((walk.total, walk.files))
}

struct TreeWalk {
    total: i64,
    files: Vec<String>,
}

/// The shared level-order walk behind [`compute_tree_size`] and
/// [`compute_tree_size_and_files`].
///
/// `path_prefix` is only consulted when `collect_files` is set; the size-only
/// path does not allocate a path per entry.
async fn walk_tree(
    repos: &Repositories,
    repo_id: &str,
    root_fs_id: &str,
    path_prefix: &str,
    collect_files: bool,
) -> Result<TreeWalk, AppError> {
    if root_fs_id == EMPTY_SHA1 {
        return Ok(TreeWalk {
            total: 0,
            files: Vec::new(),
        });
    }

    let mut total: i64 = 0;
    let mut files: Vec<String> = Vec::new();
    // Level frontier: one batched `IN` query per level (O(#dirs) → O(depth)).
    let mut frontier: Vec<(String, String)> =
        vec![(root_fs_id.to_string(), path_prefix.to_string())];

    let mut guard = crate::fs::core::traversal::TreeGuard::new();
    while !frontier.is_empty() {
        guard.enter_level()?;
        guard.visit(frontier.len())?;
        let ids: Vec<String> = frontier.iter().map(|(id, _)| id.clone()).collect();
        let dir_map = read_fs_dir_data_batch(repos, repo_id, &ids).await?;
        let mut next: Vec<(String, String)> = Vec::new();

        for (fs_id, prefix) in &frontier {
            // Missing/EMPTY dirs are absent from the batch map → skip.
            let Some(dir_data) = dir_map.get(fs_id) else {
                continue;
            };
            for entry in &dir_data.dirents {
                let child_path = if collect_files {
                    infra::common::util::join_path(prefix, &entry.name)
                } else {
                    String::new()
                };
                if entry.mode & S_IFDIR != 0 {
                    next.push((entry.id.clone(), child_path));
                } else {
                    total += entry.size;
                    if collect_files {
                        files.push(child_path);
                    }
                }
            }
        }

        frontier = next;
    }

    Ok(TreeWalk { total, files })
}

/// Resolve the directory entry at `path`, or `None` when it does not exist.
///
/// This is the common half of [`get_entry_total_size`] and [`entry_exists`]: the
/// former may then walk a directory's whole subtree, while the latter only needs
/// to know whether the entry is there. The existence path used to call the total
/// size function and throw the size away, which made an upload targeting a large
/// directory walk the entire directory first.
async fn resolve_dirent(
    repos: &Repositories,
    repo_id: &str,
    path: &str,
) -> Result<Option<base::common::DirEntryData>, AppError> {
    let parent_path = infra::common::util::parent_path_from(path);
    let name = basename(path);

    let head = crate::fs::core::repo_head(repos, repo_id).await?;
    if head.head_commit_id.is_none() {
        return Err(AppError::NotFound("no commits yet".into()));
    }

    let parent_fs_id = resolve_fs_id(repos, repo_id, &head.root_id, parent_path)
        .await
        .map_err(|e| AppError::internal(format!("resolve parent failed: {e}")))?;

    let dir_data = read_fs_dir_data(repos, repo_id, &parent_fs_id)
        .await
        .map_err(|e| AppError::internal(format!("read dir failed: {e}")))?;

    Ok(dir_data.dirents.into_iter().find(|d| d.name == name))
}

/// Total size of the entry at `path`: the stored size for a file, a recursive
/// traversal for a directory.
pub async fn get_entry_total_size(
    repos: &Repositories,
    repo_id: &str,
    path: &str,
) -> Result<i64, AppError> {
    let entry = resolve_dirent(repos, repo_id, path)
        .await?
        .ok_or_else(|| AppError::NotFound("entry not found".into()))?;

    if entry.mode & S_IFDIR != 0 {
        compute_tree_size(repos, repo_id, &entry.id).await
    } else {
        Ok(entry.size)
    }
}

/// Whether the entry at `path` exists, without computing a directory's size.
///
/// Any lookup failure is reported as "does not exist": the only caller is the
/// upload pre-check, whose existing answer for an unreadable path was 404 too.
pub async fn entry_exists(repos: &Repositories, repo_id: &str, path: &str) -> bool {
    matches!(resolve_dirent(repos, repo_id, path).await, Ok(Some(_)))
}
