use sea_orm::{
    ColumnTrait, Condition, DatabaseConnection, EntityTrait, Order, PaginatorTrait, QueryFilter,
    QueryOrder, QuerySelect, Set,
};
use serde::Serialize;
use std::collections::HashMap;

use crate::activity_log;
use crate::entity::{deleted_repo, file_trash, repo, repo_member};
use crate::error::AppError;
use crate::repo::file_ops::FileOps;
use crate::repository::Repositories;
use crate::serialization::S_IFDIR;
use base::common::DirEntryData;

/// A single item recorded during batch delete.
#[derive(Debug, Clone)]
pub struct TrashItem {
    /// Full path of the deleted item, e.g. "/Documents/report.pdf"
    pub path: String,
    pub obj_type: String, // "file" or "dir"
    pub obj_id: String,
    pub obj_name: String,
    pub size: i64,
}

/// Page-based trash listing result (for `/trash2/`).
#[derive(Debug, Serialize)]
pub struct TrashListResult {
    pub items: Vec<TrashEntry>,
    pub total_count: i64,
    pub can_search: bool,
}

/// A single trash entry in API responses.
#[derive(Debug, Clone, Serialize)]
pub struct TrashEntry {
    pub parent_dir: String,
    pub obj_name: String,
    pub deleted_time: String, // RFC 3339
    pub commit_id: String,
    pub is_dir: bool,
    pub size: i64,
    pub obj_id: String,
    pub repo_id: String,
    pub repo_name: String,
}

/// Restore operation result – matches seahub's response format.
#[derive(Debug, Serialize)]
pub struct RevertResult {
    pub success: Vec<RevertSuccessItem>,
    pub failed: Vec<RevertFailedItem>,
}

#[derive(Debug, Serialize)]
pub struct RevertSuccessItem {
    pub path: String,
    pub is_dir: bool,
}

#[derive(Debug, Serialize)]
pub struct RevertFailedItem {
    pub commit_id: String,
    pub path: String,
    pub error_msg: String,
}

/// Cursor-based trash listing result (for `/trash/`).
#[derive(Debug, Serialize)]
pub struct CursorTrashResult {
    pub items: Vec<TrashEntry>,
    pub has_more: bool,
}

/// Insert a single deleted file/dir into `file_trash`.
///
/// `path` is the full path (`/dir/file.txt`). `parent_commit_id` is the
/// commit **before** deletion (the parent still contains the entry).
///
/// Best-effort: logs and swallows errors.
#[allow(clippy::too_many_arguments)]
pub async fn add_to_trash(
    _db: &DatabaseConnection,
    repos: &Repositories,
    repo_id: &str,
    full_path: &str,
    obj_type: &str,
    obj_id: &str,
    obj_name: &str,
    size: i64,
    parent_commit_id: &str,
    user_email: &str,
) -> Result<(), AppError> {
    let parent_dir = match full_path.rsplit_once('/') {
        Some(("", _)) => "/",
        Some((parent, _)) => parent,
        None => "/",
    };

    let now = chrono::Utc::now().timestamp();

    repos
        .file_trash
        .insert(file_trash::ActiveModel {
            id: sea_orm::NotSet,
            user: Set(user_email.to_owned()),
            obj_type: Set(obj_type.to_owned()),
            obj_id: Set(obj_id.to_owned()),
            obj_name: Set(obj_name.to_owned()),
            delete_time: Set(now),
            repo_id: Set(repo_id.to_owned()),
            commit_id: Set(parent_commit_id.to_owned()),
            path: Set(parent_dir.to_owned()),
            size: Set(size),
        })
        .await?;

    Ok(())
}

/// Insert multiple deleted items into `file_trash` in a single batch.
///
/// Best-effort: logs and swallows errors.
pub async fn add_batch_to_trash(
    _db: &DatabaseConnection,
    repos: &Repositories,
    repo_id: &str,
    items: Vec<TrashItem>,
    parent_commit_id: &str,
    user_email: &str,
) -> Result<(), AppError> {
    let now = chrono::Utc::now().timestamp();

    for item in &items {
        let parent_dir = match item.path.rsplit_once('/') {
            Some(("", _)) => "/",
            Some((parent, _)) => parent,
            None => "/",
        };

        repos
            .file_trash
            .insert(file_trash::ActiveModel {
                id: sea_orm::NotSet,
                user: Set(user_email.to_owned()),
                obj_type: Set(item.obj_type.to_owned()),
                obj_id: Set(item.obj_id.to_owned()),
                obj_name: Set(item.obj_name.to_owned()),
                delete_time: Set(now),
                repo_id: Set(repo_id.to_owned()),
                commit_id: Set(parent_commit_id.to_owned()),
                path: Set(parent_dir.to_owned()),
                size: Set(item.size),
            })
            .await?;
    }

    Ok(())
}

/// Page-based trash listing for the `/trash2/` endpoint.
pub async fn list_trash2(
    _db: &DatabaseConnection,
    repos: &Repositories,
    repo_id: &str,
    page: u32,
    per_page: u32,
) -> Result<TrashListResult, AppError> {
    let page = page.max(1);
    let per_page = per_page.clamp(1, 100);
    let offset = ((page - 1) * per_page) as u64;

    // Count total
    let total_count = repos.file_trash.count_by_repo(repo_id).await?;

    // Fetch items
    let rows = repos
        .file_trash
        .find_by_repo_paginated(repo_id, per_page as u64, offset)
        .await?;

    let items = rows
        .iter()
        .map(|m| TrashEntry {
            parent_dir: m.path.clone(),
            obj_name: m.obj_name.clone(),
            deleted_time: chrono::DateTime::from_timestamp(m.delete_time, 0)
                .map(|d| d.to_rfc3339())
                .unwrap_or_default(),
            commit_id: m.commit_id.clone(),
            is_dir: m.obj_type == "dir",
            size: m.size,
            obj_id: m.obj_id.clone(),
            repo_id: repo_id.to_string(),
            repo_name: String::new(),
        })
        .collect();

    Ok(TrashListResult {
        items,
        total_count,
        can_search: true,
    })
}

/// Search trash by keyword and filters.
#[allow(clippy::too_many_arguments)]
pub async fn search_trash(
    db: &DatabaseConnection,
    repo_id: &str,
    query: &str,
    page: u32,
    per_page: u32,
    op_users: Option<&str>,
    time_from: Option<i64>,
    time_to: Option<i64>,
    suffixes: Option<&str>,
) -> Result<TrashListResult, AppError> {
    let page = page.max(1);
    let per_page = per_page.clamp(1, 100);
    let offset = ((page - 1) * per_page) as u64;

    let mut condition = Condition::all().add(file_trash::Column::RepoId.eq(repo_id.to_owned()));

    // Keyword search on obj_name
    if !query.is_empty() {
        condition = condition.add(file_trash::Column::ObjName.contains(query));
    }

    // Filter by users
    if let Some(users) = op_users
        && !users.is_empty()
    {
        let emails: Vec<String> = users
            .split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();
        if !emails.is_empty() {
            condition = condition.add(file_trash::Column::User.is_in(emails));
        }
    }

    // Time range filter
    if let Some(from) = time_from {
        condition = condition.add(file_trash::Column::DeleteTime.gte(from));
    }
    if let Some(to) = time_to {
        condition = condition.add(file_trash::Column::DeleteTime.lte(to));
    }

    // File extension filter
    if let Some(suffixes_str) = suffixes
        && !suffixes_str.is_empty()
    {
        let exts: Vec<&str> = suffixes_str
            .split(',')
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
            .collect();
        if !exts.is_empty() {
            let mut any = Condition::any();
            for ext in &exts {
                let pattern = if ext.starts_with('.') {
                    format!("%{}", ext)
                } else {
                    format!("%.{}", ext)
                };
                any = any.add(file_trash::Column::ObjName.like(pattern));
            }
            condition = condition.add(any);
        }
    }

    // Count
    let total_count = file_trash::Entity::find()
        .filter(condition.clone())
        .count(db)
        .await? as i64;

    // Fetch items
    let rows = file_trash::Entity::find()
        .filter(condition)
        .order_by(file_trash::Column::DeleteTime, Order::Desc)
        .limit(per_page as u64)
        .offset(offset)
        .all(db)
        .await?;

    let items = rows
        .iter()
        .map(|m| TrashEntry {
            parent_dir: m.path.clone(),
            obj_name: m.obj_name.clone(),
            deleted_time: chrono::DateTime::from_timestamp(m.delete_time, 0)
                .map(|d| d.to_rfc3339())
                .unwrap_or_default(),
            commit_id: m.commit_id.clone(),
            is_dir: m.obj_type == "dir",
            size: m.size,
            obj_id: m.obj_id.clone(),
            repo_id: repo_id.to_string(),
            repo_name: String::new(),
        })
        .collect();

    Ok(TrashListResult {
        items,
        total_count,
        can_search: true,
    })
}

/// Cursor-based trash listing for the `/trash/` endpoint.
///
/// `cursor` is an optional `delete_time` value from the last item of
/// the previous page. Returns items with `delete_time <= cursor`. When
/// `cursor` is `None`, returns the most recent items.
pub async fn list_trash_cursor(
    _db: &DatabaseConnection,
    repos: &Repositories,
    repo_id: &str,
    cursor: Option<i64>,
    limit: u32,
) -> Result<CursorTrashResult, AppError> {
    let limit = limit.clamp(1, 100);
    let fetch = (limit + 1) as u64; // fetch one extra to detect has_more

    let rows = repos
        .file_trash
        .find_by_repo_cursor(repo_id, cursor, fetch)
        .await?;

    let has_more = rows.len() > limit as usize;
    let items: Vec<TrashEntry> = rows
        .into_iter()
        .take(limit as usize)
        .map(|m| TrashEntry {
            parent_dir: m.path,
            obj_name: m.obj_name,
            deleted_time: chrono::DateTime::from_timestamp(m.delete_time, 0)
                .map(|d| d.to_rfc3339())
                .unwrap_or_default(),
            commit_id: m.commit_id,
            is_dir: m.obj_type == "dir",
            size: m.size,
            obj_id: m.obj_id,
            repo_id: repo_id.to_string(),
            repo_name: String::new(),
        })
        .collect();

    Ok(CursorTrashResult { items, has_more })
}

/// Remove trash records by their primary key IDs.
async fn delete_trash_records(repos: &Repositories, ids: &[i32]) -> Result<(), AppError> {
    repos.file_trash.delete_by_ids(ids).await
}

/// Check whether a path exists in the current FS tree (i.e. the resolved
/// fs_id from the repo's head commit actually exists).
async fn path_exists_in_tree(
    _db: &DatabaseConnection,
    repos: &Repositories,
    repo_id: &str,
    path: &str,
) -> Result<bool, AppError> {
    if path == "/" {
        return Ok(true);
    }

    let repo_record = repos
        .repo
        .find_by_id(repo_id)
        .await?
        .ok_or_else(|| AppError::NotFound("repo not found".into()))?;
    let head_commit_id = match repo_record.head_commit_id {
        Some(id) => id,
        None => return Ok(false),
    };
    let head = repos
        .commit
        .find_by_repo_and_commit_id(repo_id, &head_commit_id)
        .await?
        .ok_or_else(|| AppError::NotFound("head commit not found".into()))?;

    match crate::repo::resolve_fs_id(repos, repo_id, &head.root_id, path).await {
        Ok(_) => Ok(true),
        Err(_) => Ok(false),
    }
}

/// Check whether a file/dir name already exists in a parent directory in
/// the current FS tree.
async fn name_exists_in_parent(
    _db: &DatabaseConnection,
    repos: &Repositories,
    repo_id: &str,
    parent_path: &str,
    name: &str,
) -> Result<bool, AppError> {
    let repo_record = repos
        .repo
        .find_by_id(repo_id)
        .await?
        .ok_or_else(|| AppError::NotFound("repo not found".into()))?;
    let head_commit_id = match repo_record.head_commit_id {
        Some(id) => id,
        None => return Ok(false),
    };
    let head = repos
        .commit
        .find_by_repo_and_commit_id(repo_id, &head_commit_id)
        .await?
        .ok_or_else(|| AppError::NotFound("head commit not found".into()))?;

    // Resolve parent directory fs_id
    let parent_fs_id =
        match crate::repo::resolve_fs_id(repos, repo_id, &head.root_id, parent_path).await {
            Ok(id) => id,
            Err(_) => return Ok(false),
        };

    // Read parent directory entries
    let parent_data = match crate::repo::read_fs_dir_data(repos, repo_id, &parent_fs_id).await {
        Ok(data) => data,
        Err(_) => return Ok(false),
    };

    Ok(parent_data.dirents.iter().any(|d| d.name == name))
}

/// Verify that an fs_object exists in the database.
async fn fs_object_exists(
    repos: &Repositories,
    repo_id: &str,
    fs_id: &str,
) -> Result<bool, AppError> {
    if fs_id == crate::common::EMPTY_SHA1 {
        return Ok(true);
    }
    repos
        .fs_object
        .exists_by_repo_and_fs_id(repo_id, fs_id)
        .await
}

/// Core restore logic. Takes a map of `commit_id -> [paths]` where paths
/// are the full paths (matching `parent_dir + "/" + obj_name` in trash).
///
/// Returns a `RevertResult` with success and failed items.
pub async fn restore_trash_items(
    db: &DatabaseConnection,
    repos: &Repositories,
    repo_id: &str,
    modifier: &str,
    user_id: i32,
    restore_map: HashMap<String, Vec<String>>,
) -> Result<RevertResult, AppError> {
    let mut success = Vec::new();
    let mut failed = Vec::new();

    for (commit_id, paths) in &restore_map {
        for full_path in paths {
            let full_path = full_path.trim_end_matches('/');

            // Split full_path into parent_dir and obj_name
            let (parent_dir, obj_name) = match full_path.rsplit_once('/') {
                Some(("", name)) => ("/", name),
                Some((parent, name)) => (parent, name),
                None => ("/", full_path),
            };

            // Find the trash record
            let Some(model) = repos
                .file_trash
                .find_by_compound_key(repo_id, commit_id, parent_dir, obj_name)
                .await?
            else {
                failed.push(RevertFailedItem {
                    commit_id: commit_id.clone(),
                    path: full_path.to_string(),
                    error_msg: format!("Dirent {full_path} not found."),
                });
                continue;
            };

            let trash_id = model.id;
            let obj_type = model.obj_type.clone();
            let obj_id = model.obj_id.clone();
            let is_dir = obj_type == "dir";

            // Verify fs_object exists
            if !fs_object_exists(repos, repo_id, &obj_id).await? {
                failed.push(RevertFailedItem {
                    commit_id: commit_id.clone(),
                    path: full_path.to_string(),
                    error_msg: "Object not found.".into(),
                });
                continue;
            }

            // Verify parent directory exists in current tree
            if parent_dir != "/" && !path_exists_in_tree(db, repos, repo_id, parent_dir).await? {
                failed.push(RevertFailedItem {
                    commit_id: commit_id.clone(),
                    path: full_path.to_string(),
                    error_msg: format!("Directory {parent_dir} not found."),
                });
                continue;
            }

            // Check for name collision
            if name_exists_in_parent(db, repos, repo_id, parent_dir, obj_name).await? {
                failed.push(RevertFailedItem {
                    commit_id: commit_id.clone(),
                    path: full_path.to_string(),
                    error_msg: "A file with the same name already exists.".to_string(),
                });
                continue;
            }

            // Resolve parent fs_id from current tree
            let repo_record = repos
                .repo
                .find_by_id(repo_id)
                .await?
                .ok_or_else(|| AppError::NotFound("repo not found".into()))?;
            let head_commit_id = repo_record
                .head_commit_id
                .ok_or_else(|| AppError::NotFound("no commits".into()))?;
            let head = repos
                .commit
                .find_by_repo_and_commit_id(repo_id, &head_commit_id)
                .await?
                .ok_or_else(|| AppError::NotFound("head commit not found".into()))?;

            let parent_fs_id = if parent_dir == "/" {
                head.root_id.clone()
            } else {
                match crate::repo::resolve_fs_id(repos, repo_id, &head.root_id, parent_dir).await {
                    Ok(id) => id,
                    Err(_) => {
                        failed.push(RevertFailedItem {
                            commit_id: commit_id.clone(),
                            path: full_path.to_string(),
                            error_msg: format!("Directory {parent_dir} not found."),
                        });
                        continue;
                    }
                }
            };

            if parent_fs_id == crate::common::EMPTY_SHA1 {
                failed.push(RevertFailedItem {
                    commit_id: commit_id.clone(),
                    path: full_path.to_string(),
                    error_msg: format!("Directory {parent_dir} not found."),
                });
                continue;
            }

            let now = chrono::Utc::now().timestamp();

            // Get the size from the stored trash record for adjustments later
            let entry_size = model.size;
            let _ = entry_size; // future use

            // Insert entry into parent directory and create commit
            let description = format!("Recovered {obj_name}");

            let result = FileOps::update_dir_tree_and_commit(
                db,
                repos,
                repo_id,
                parent_dir,
                &parent_fs_id,
                modifier,
                &description,
                crate::repo::file_ops::EMPTY_ANCESTOR_CHAIN,
                |dirents| {
                    dirents.push(DirEntryData {
                        id: obj_id.clone(),
                        mode: if is_dir {
                            S_IFDIR
                        } else {
                            crate::serialization::S_IFREG
                        },
                        modifier: modifier.to_string(),
                        mtime: now,
                        name: obj_name.to_string(),
                        size: entry_size,
                    });
                    Ok(())
                },
            )
            .await
            .map_err(|e| AppError::Internal(format!("Restore commit failed: {e}")));

            match result {
                Ok(_) => {
                    // Delete the trash record
                    delete_trash_records(repos, &[trash_id]).await?;

                    // Log activity
                    activity_log::log_activity(
                        db,
                        repo_id,
                        "recover",
                        if is_dir { "dir" } else { "file" },
                        full_path,
                        user_id,
                        None,
                        Some(entry_size),
                        Some(&obj_id),
                        None,
                        None,
                    )
                    .await;

                    success.push(RevertSuccessItem {
                        path: full_path.to_string(),
                        is_dir,
                    });
                }
                Err(e) => {
                    failed.push(RevertFailedItem {
                        commit_id: commit_id.clone(),
                        path: full_path.to_string(),
                        error_msg: format!("Restore failed: {e}"),
                    });
                }
            }
        }
    }

    Ok(RevertResult { success, failed })
}

/// Old API restore: single commit_id, multiple paths.
pub async fn restore_dirents(
    db: &DatabaseConnection,
    repos: &Repositories,
    repo_id: &str,
    modifier: &str,
    user_id: i32,
    commit_id: &str,
    paths: Vec<String>,
) -> Result<RevertResult, AppError> {
    let mut map = HashMap::new();
    map.insert(commit_id.to_string(), paths);
    restore_trash_items(db, repos, repo_id, modifier, user_id, map).await
}

/// Clean trash items for a repo, optionally keeping items newer than
/// `keep_days`. Returns the number of deleted rows.
pub async fn clean_trash(
    _db: &DatabaseConnection,
    repos: &Repositories,
    repo_id: &str,
    keep_days: Option<i64>,
) -> Result<u64, AppError> {
    let cutoff = keep_days
        .filter(|d| *d > 0)
        .map(|d| chrono::Utc::now().timestamp() - d * 86400);

    if let Some(c) = cutoff {
        repos.file_trash.delete_by_repo_before(repo_id, c).await?;
    } else {
        repos.file_trash.delete_by_repo(repo_id).await?;
    }

    // The repository methods don't return row counts; return 0 as best-effort.
    Ok(0)
}

/// List trash items across all repos the user has access to.
///
/// Joins with `repo_members` to find accessible repos, and `repos` to
/// include the repo name for display.
pub async fn list_trash_for_user(
    db: &DatabaseConnection,
    repos: &Repositories,
    user_id: i32,
    page: u32,
    per_page: u32,
) -> Result<TrashListResult, AppError> {
    let page = page.max(1);
    let per_page = per_page.clamp(1, 100);
    let offset = ((page - 1) * per_page) as u64;

    // Gather repo_ids accessible by the user
    let member_repos = repos.member.find_by_user_id(user_id).await?;
    let repo_ids: Vec<String> = member_repos
        .into_iter()
        .map(|m| m.repo_id)
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect();

    if repo_ids.is_empty() {
        return Ok(TrashListResult {
            items: Vec::new(),
            total_count: 0,
            can_search: true,
        });
    }

    // Count
    let total_count = file_trash::Entity::find()
        .filter(file_trash::Column::RepoId.is_in(repo_ids.clone()))
        .count(db)
        .await? as i64;

    // Build repo name lookup
    let mut repo_names: HashMap<String, String> = HashMap::new();
    for rid in &repo_ids {
        if let Some(r) = repos.repo.find_by_id(rid).await? {
            repo_names.insert(rid.clone(), r.name);
        }
    }

    // Fetch items
    let rows = file_trash::Entity::find()
        .filter(file_trash::Column::RepoId.is_in(repo_ids))
        .order_by(file_trash::Column::DeleteTime, Order::Desc)
        .limit(per_page as u64)
        .offset(offset)
        .all(db)
        .await?;

    let items = rows
        .iter()
        .map(|m| {
            let repo_name = repo_names.get(&m.repo_id).cloned().unwrap_or_default();
            TrashEntry {
                parent_dir: m.path.clone(),
                obj_name: m.obj_name.clone(),
                deleted_time: chrono::DateTime::from_timestamp(m.delete_time, 0)
                    .map(|d| d.to_rfc3339())
                    .unwrap_or_default(),
                commit_id: m.commit_id.clone(),
                is_dir: m.obj_type == "dir",
                size: m.size,
                obj_id: m.obj_id.clone(),
                repo_id: m.repo_id.clone(),
                repo_name,
            }
        })
        .collect();

    Ok(TrashListResult {
        items,
        total_count,
        can_search: true,
    })
}

/// Search trash across all repos the user has access to.
///
/// Supports the same filters as `search_trash` but scoped to repos
/// the user can access via `repo_members`.
#[allow(clippy::too_many_arguments)]
pub async fn search_trash_for_user(
    db: &DatabaseConnection,
    repos: &Repositories,
    user_id: i32,
    query: &str,
    page: u32,
    per_page: u32,
    time_from: Option<i64>,
    time_to: Option<i64>,
    suffixes: Option<&str>,
) -> Result<TrashListResult, AppError> {
    let page = page.max(1);
    let per_page = per_page.clamp(1, 100);
    let offset = ((page - 1) * per_page) as u64;

    // Gather repo_ids accessible by the user
    let member_repos = repos.member.find_by_user_id(user_id).await?;
    let repo_ids: Vec<String> = member_repos
        .into_iter()
        .map(|m| m.repo_id)
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect();

    if repo_ids.is_empty() {
        return Ok(TrashListResult {
            items: Vec::new(),
            total_count: 0,
            can_search: true,
        });
    }

    let mut condition = Condition::all().add(file_trash::Column::RepoId.is_in(repo_ids.clone()));

    // Keyword search on obj_name
    if !query.is_empty() {
        condition = condition.add(file_trash::Column::ObjName.contains(query));
    }

    // Time range filter
    if let Some(from) = time_from {
        condition = condition.add(file_trash::Column::DeleteTime.gte(from));
    }
    if let Some(to) = time_to {
        condition = condition.add(file_trash::Column::DeleteTime.lte(to));
    }

    // File extension filter
    if let Some(suffixes_str) = suffixes
        && !suffixes_str.is_empty()
    {
        let exts: Vec<&str> = suffixes_str
            .split(',')
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
            .collect();
        if !exts.is_empty() {
            let mut any = Condition::any();
            for ext in &exts {
                let pattern = if ext.starts_with('.') {
                    format!("%{}", ext)
                } else {
                    format!("%.{}", ext)
                };
                any = any.add(file_trash::Column::ObjName.like(pattern));
            }
            condition = condition.add(any);
        }
    }

    // Count
    let total_count = file_trash::Entity::find()
        .filter(condition.clone())
        .count(db)
        .await? as i64;

    // Build repo name lookup
    let mut repo_names: HashMap<String, String> = HashMap::new();
    for rid in &repo_ids {
        if let Some(r) = repos.repo.find_by_id(rid).await? {
            repo_names.insert(rid.clone(), r.name);
        }
    }

    // Fetch items
    let rows = file_trash::Entity::find()
        .filter(condition)
        .order_by(file_trash::Column::DeleteTime, Order::Desc)
        .limit(per_page as u64)
        .offset(offset)
        .all(db)
        .await?;

    let items = rows
        .iter()
        .map(|m| {
            let repo_name = repo_names.get(&m.repo_id).cloned().unwrap_or_default();
            TrashEntry {
                parent_dir: m.path.clone(),
                obj_name: m.obj_name.clone(),
                deleted_time: chrono::DateTime::from_timestamp(m.delete_time, 0)
                    .map(|d| d.to_rfc3339())
                    .unwrap_or_default(),
                commit_id: m.commit_id.clone(),
                is_dir: m.obj_type == "dir",
                size: m.size,
                obj_id: m.obj_id.clone(),
                repo_id: m.repo_id.clone(),
                repo_name,
            }
        })
        .collect();

    Ok(TrashListResult {
        items,
        total_count,
        can_search: true,
    })
}

// ─── Repo-level trash ─────────────────────────────────────────────────

/// Add a deleted repo to the trash table.
pub async fn add_deleted_repo(
    _db: &DatabaseConnection,
    repos: &Repositories,
    repo_id: &str,
    repo_name: &str,
    head_id: Option<&str>,
    owner_id: i32,
    size: i64,
) -> Result<(), AppError> {
    let now = chrono::Utc::now().timestamp();

    // Best-effort: if the repo already exists in trash, ignore the error
    // to preserve the original INSERT OR IGNORE behavior.
    if repos.deleted_repo.find_by_id(repo_id).await?.is_none() {
        repos
            .deleted_repo
            .insert(deleted_repo::ActiveModel {
                repo_id: Set(repo_id.to_owned()),
                repo_name: Set(repo_name.to_owned()),
                head_id: Set(head_id.map(|s| s.to_owned())),
                owner_id: Set(owner_id),
                size: Set(size),
                del_time: Set(now),
            })
            .await?;
    }

    Ok(())
}

/// List repos that a user has deleted.
pub async fn list_deleted_repos(
    repos: &Repositories,
    user_id: i32,
) -> Result<Vec<deleted_repo::Model>, AppError> {
    repos.deleted_repo.find_by_owner(user_id).await
}

/// Restore a repo from trash.
///
/// Re-inserts the repo, creates owner membership, and removes from trash.
pub async fn restore_deleted_repo(
    db: &DatabaseConnection,
    repos: &Repositories,
    repo_id: &str,
    user_id: i32,
) -> Result<(), AppError> {
    let trashed = repos
        .deleted_repo
        .find_by_id(repo_id)
        .await?
        .ok_or_else(|| AppError::NotFound("repo not found in trash".into()))?;

    if trashed.owner_id != user_id {
        return Err(AppError::Forbidden);
    }

    let now = chrono::Utc::now().timestamp();

    // Re-insert repo (with INSERT OR IGNORE semantics — skip if already exists)
    if repos.repo.find_by_id(&trashed.repo_id).await?.is_none() {
        repos
            .repo
            .create(repo::ActiveModel {
                id: Set(trashed.repo_id.clone()),
                name: Set(trashed.repo_name.clone()),
                description: Set(String::new()),
                owner_id: Set(trashed.owner_id),
                encrypted: Set(0i8),
                enc_version: Set(0i8),
                magic: Set(None),
                random_key: Set(None),
                salt: Set(String::new()),
                head_commit_id: Set(None),
                permission: Set("rw".to_string()),
                created_at: Set(now),
                updated_at: Set(now),
                size: Set(trashed.size),
                repo_version: Set(1i32),
            })
            .await?;
    }

    // Re-create owner membership (with INSERT OR IGNORE semantics)
    if repos
        .member
        .find_by_repo_and_user(&trashed.repo_id, trashed.owner_id)
        .await?
        .is_none()
    {
        repos
            .member
            .create(repo_member::ActiveModel {
                id: Set(0i32),
                repo_id: Set(trashed.repo_id.clone()),
                user_id: Set(trashed.owner_id),
                permission: Set("rw".to_string()),
                created_at: Set(now),
            })
            .await?;
    }

    // Remove from trash
    repos.deleted_repo.delete_by_id(repo_id).await?;

    // Log activity
    activity_log::log_activity(
        db, repo_id, "recover", "repo", "/", user_id, None, None, None, None, None,
    )
    .await;

    Ok(())
}

/// Permanently delete a repo from trash (admin).
pub async fn permanently_delete_deleted_repo(
    repos: &Repositories,
    repo_id: &str,
) -> Result<(), AppError> {
    repos.deleted_repo.delete_by_id(repo_id).await?;
    Ok(())
}
