use sea_orm::{ActiveModelTrait, ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter, Set};

use crate::common::EMPTY_SHA1;
use crate::entity::{commit, repo};
use crate::error::AppError;
use crate::serialization::S_IFDIR;

use super::fs_tree::{read_fs_dir_data, resolve_fs_id};

/// Compute the total size of all files in a repository by recursively
/// traversing the FS tree from the repo's head commit.
///
/// Returns 0 if the repo has no commits yet or the tree is empty.
pub async fn compute_repo_size(db: &DatabaseConnection, repo_id: &str) -> Result<i64, AppError> {
    let repo_record = repo::Entity::find_by_id(repo_id)
        .one(db)
        .await?
        .ok_or_else(|| AppError::NotFound("repo not found".into()))?;

    let head_commit_id = match repo_record.head_commit_id {
        Some(id) => id,
        None => return Ok(0),
    };

    let head = commit::Entity::find()
        .filter(commit::Column::RepoId.eq(repo_id))
        .filter(commit::Column::CommitId.eq(&head_commit_id))
        .one(db)
        .await?
        .ok_or_else(|| AppError::NotFound("head commit not found".into()))?;

    compute_tree_size(db, repo_id, &head.root_id).await
}

/// Recursively walk a directory tree (starting at root_fs_id) and sum
/// up all file sizes.
pub async fn compute_tree_size(
    db: &DatabaseConnection,
    repo_id: &str,
    root_fs_id: &str,
) -> Result<i64, AppError> {
    use std::collections::VecDeque;

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

        let dir_data = match read_fs_dir_data(db, repo_id, &fs_id).await {
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
    db: &DatabaseConnection,
    repo_id: &str,
    delta: i64,
) -> Result<(), AppError> {
    let r = repo::Entity::find_by_id(repo_id)
        .one(db)
        .await?
        .ok_or_else(|| AppError::NotFound("repo not found".into()))?;

    let current_size = r.size;
    if current_size == 0 {
        let size = compute_repo_size(db, repo_id).await?;
        let mut active: repo::ActiveModel = r.into();
        active.size = Set(size);
        active.update(db).await?;
    } else {
        let new_size = (current_size + delta).max(0);
        let mut active: repo::ActiveModel = r.into();
        active.size = Set(new_size);
        active.update(db).await?;
    }
    Ok(())
}

/// Look up a single file/dir entry in the FS tree and return its total
/// size.  For a file this is the `DirEntryData.size` field; for a
/// directory the subtree is walked recursively via `compute_tree_size`.
pub async fn get_entry_total_size(
    db: &DatabaseConnection,
    repo_id: &str,
    path: &str,
) -> Result<i64, AppError> {
    let parent_path = match path.rsplit_once('/') {
        Some(("", _)) => "/",
        Some((parent, _)) => parent,
        None => "/",
    };
    let name = path.rsplit_once('/').map(|(_, n)| n).unwrap_or("");

    let repo_record = repo::Entity::find_by_id(repo_id)
        .one(db)
        .await?
        .ok_or_else(|| AppError::NotFound("repo not found".into()))?;
    let head_commit_id = repo_record
        .head_commit_id
        .ok_or_else(|| AppError::NotFound("no commits yet".into()))?;
    let head = commit::Entity::find()
        .filter(commit::Column::RepoId.eq(repo_id))
        .filter(commit::Column::CommitId.eq(&head_commit_id))
        .one(db)
        .await?
        .ok_or_else(|| AppError::NotFound("head commit not found".into()))?;

    let parent_fs_id = resolve_fs_id(db, repo_id, &head.root_id, parent_path)
        .await
        .map_err(|e| AppError::internal(format!("resolve parent failed: {e}")))?;

    let dir_data = read_fs_dir_data(db, repo_id, &parent_fs_id)
        .await
        .map_err(|e| AppError::internal(format!("read dir failed: {e}")))?;

    let entry = dir_data
        .dirents
        .iter()
        .find(|d| d.name == name)
        .ok_or_else(|| AppError::NotFound("entry not found".into()))?;

    if entry.mode & S_IFDIR != 0 {
        compute_tree_size(db, repo_id, &entry.id).await
    } else {
        Ok(entry.size)
    }
}
