use std::collections::VecDeque;

use crate::repository::Repositories;
use base::error::AppError;
use infra::common::EMPTY_SHA1;
use infra::serialization::S_IFDIR;

use crate::fs::core::{read_fs_dir_data, resolve_fs_id};

/// Compute the total size of all files in a repository by recursively
/// traversing the FS tree from the repo's head commit.
///
/// Returns 0 if the repo has no commits yet or the tree is empty.
pub async fn compute_repo_size(repos: &Repositories, repo_id: &str) -> Result<i64, AppError> {
    let repo_record = repos
        .repo
        .find_by_id(repo_id)
        .await?
        .ok_or_else(|| AppError::NotFound("repo not found".into()))?;

    let head_commit_id = match repo_record.head_commit_id {
        Some(id) => id,
        None => return Ok(0),
    };

    let head = repos
        .commit
        .find_by_repo_and_commit_id(repo_id, &head_commit_id)
        .await?
        .ok_or_else(|| AppError::NotFound("head commit not found".into()))?;

    compute_tree_size(repos, repo_id, &head.root_id).await
}

/// Recursively walk a directory tree (starting at root_fs_id) and sum
/// up all file sizes.
pub async fn compute_tree_size(
    repos: &Repositories,
    repo_id: &str,
    root_fs_id: &str,
) -> Result<i64, AppError> {
    if root_fs_id == EMPTY_SHA1 {
        return Ok(0);
    }

    let mut total: i64 = 0;
    let mut queue = VecDeque::new();
    queue.push_back(root_fs_id.to_string());

    while let Some(fs_id) = queue.pop_front() {
        if fs_id == EMPTY_SHA1 {
            continue;
        }

        let dir_data = match read_fs_dir_data(repos, repo_id, &fs_id).await {
            Ok(d) => d,
            Err(_) => continue,
        };

        for entry in &dir_data.dirents {
            if entry.mode & S_IFDIR != 0 {
                queue.push_back(entry.id.clone());
            } else {
                total += entry.size;
            }
        }
    }

    Ok(total)
}

/// Adjust a repo's stored size by `delta` bytes.
///
/// If `repo.size` is 0 (never computed), falls back to a full traversal
/// via `compute_repo_size()` to get a correct baseline.  Otherwise
/// applies the delta incrementally.
pub async fn adjust_repo_size(
    repos: &Repositories,
    repo_id: &str,
    delta: i64,
) -> Result<(), AppError> {
    let r = repos
        .repo
        .find_by_id(repo_id)
        .await?
        .ok_or_else(|| AppError::NotFound("repo not found".into()))?;

    let current_size = r.size;
    if current_size == 0 {
        let size = compute_repo_size(repos, repo_id).await?;
        repos.repo.adjust_size(repo_id, size).await?;
    } else {
        repos.repo.adjust_size(repo_id, delta).await?;
    }
    Ok(())
}

/// Look up a single file/dir entry in the FS tree and return its total
/// size.  For a file this is the `DirEntryData.size` field; for a
/// directory the subtree is walked recursively via `compute_tree_size`.
pub async fn get_entry_total_size(
    repos: &Repositories,
    repo_id: &str,
    path: &str,
) -> Result<i64, AppError> {
    let parent_path = match path.rsplit_once('/') {
        Some(("", _)) => "/",
        Some((parent, _)) => parent,
        None => "/",
    };
    let name = path.rsplit_once('/').map(|(_, n)| n).unwrap_or("");

    let repo_record = repos
        .repo
        .find_by_id(repo_id)
        .await?
        .ok_or_else(|| AppError::NotFound("repo not found".into()))?;
    let head_commit_id = repo_record
        .head_commit_id
        .ok_or_else(|| AppError::NotFound("no commits yet".into()))?;
    let head = repos
        .commit
        .find_by_repo_and_commit_id(repo_id, &head_commit_id)
        .await?
        .ok_or_else(|| AppError::NotFound("head commit not found".into()))?;

    let parent_fs_id = resolve_fs_id(repos, repo_id, &head.root_id, parent_path)
        .await
        .map_err(|e| AppError::internal(format!("resolve parent failed: {e}")))?;

    let dir_data = read_fs_dir_data(repos, repo_id, &parent_fs_id)
        .await
        .map_err(|e| AppError::internal(format!("read dir failed: {e}")))?;

    let entry = dir_data
        .dirents
        .iter()
        .find(|d| d.name == name)
        .ok_or_else(|| AppError::NotFound("entry not found".into()))?;

    if entry.mode & S_IFDIR != 0 {
        compute_tree_size(repos, repo_id, &entry.id).await
    } else {
        Ok(entry.size)
    }
}
