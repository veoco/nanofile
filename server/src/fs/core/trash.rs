use sea_orm::{
    ColumnTrait, Condition, DatabaseConnection, EntityTrait, Order, PaginatorTrait, QueryFilter,
    QueryOrder, QuerySelect, Set,
};
use serde::Serialize;
use std::collections::HashMap;

use crate::fs::core::file_ops::FileOps;
use crate::repository::Repositories;
use base::common::DirEntryData;
use base::error::AppError;
use infra::activity_log;
use infra::common::util::{get_head_commit_id, timestamp_rfc3339};
use infra::entity::{deleted_repo, file_trash, repo, repo_member};
use infra::serialization::S_IFDIR;

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
    pub deleted_time_ts: i64, // Unix seconds
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

/// Record a single deleted entry in the trash table, resolving its id, size
/// and type from the parent directory data.
///
/// Best-effort: failures are logged and swallowed so a trash-record problem
/// never blocks the actual deletion.
pub async fn record_deleted_entry(
    db: &DatabaseConnection,
    repos: &Repositories,
    repo_id: &str,
    full_path: &str,
    name: &str,
    user_email: &str,
    parent_fs_id: &str,
) {
    let head_commit_id = match get_head_commit_id(db, repo_id).await {
        Ok(id) => id,
        Err(_) => return,
    };
    let parent_dir_data =
        match crate::fs::core::read_fs_dir_data(repos, repo_id, parent_fs_id).await {
            Ok(d) => d,
            Err(_) => return,
        };
    let Some(entry) = parent_dir_data.dirents.iter().find(|d| d.name == name) else {
        return;
    };
    let obj_type = if entry.mode & S_IFDIR != 0 {
        "dir"
    } else {
        "file"
    };
    if let Err(e) = add_to_trash(
        repos,
        repo_id,
        full_path,
        obj_type,
        &entry.id,
        &entry.name,
        entry.size,
        &head_commit_id,
        user_email,
    )
    .await
    {
        tracing::warn!("Failed to record trash for {full_path}: {e}");
    }
}

/// Insert multiple deleted items into `file_trash` in a single batch.
///
/// Best-effort: logs and swallows errors.
pub async fn add_batch_to_trash(
    repos: &Repositories,
    repo_id: &str,
    items: Vec<TrashItem>,
    parent_commit_id: &str,
    user_email: &str,
) -> Result<(), AppError> {
    let now = chrono::Utc::now().timestamp();

    let models: Vec<file_trash::ActiveModel> = items
        .iter()
        .map(|item| {
            let parent_dir = match item.path.rsplit_once('/') {
                Some(("", _)) => "/",
                Some((parent, _)) => parent,
                None => "/",
            };
            file_trash::ActiveModel {
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
            }
        })
        .collect();

    repos.file_trash.insert_many(models).await
}

// ── Shared helpers ────────────────────────────────────────────────────

/// Build a condition for trash queries applying keyword, time range,
/// suffix, and optional user filters on top of a repo filter condition.
fn build_trash_condition(
    repo_condition: Condition,
    query: &str,
    time_from: Option<i64>,
    time_to: Option<i64>,
    suffixes: Option<&str>,
    op_users: Option<&str>,
) -> Condition {
    let mut condition = Condition::all().add(repo_condition);

    if !query.is_empty() {
        condition = condition.add(file_trash::Column::ObjName.contains(query));
    }

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

    if let Some(from) = time_from {
        condition = condition.add(file_trash::Column::DeleteTime.gte(from));
    }
    if let Some(to) = time_to {
        condition = condition.add(file_trash::Column::DeleteTime.lte(to));
    }

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

    condition
}

/// Map trash DB rows to `TrashEntry` response values.
///
/// When `repo_names` is `Some`, the `repo_name` field is populated from the
/// lookup table. Otherwise it is left empty.
fn map_trash_rows_to_entries(
    rows: &[file_trash::Model],
    repo_names: Option<&HashMap<String, String>>,
) -> Vec<TrashEntry> {
    rows.iter()
        .map(|m| {
            let repo_name = repo_names
                .and_then(|names| names.get(&m.repo_id).cloned())
                .unwrap_or_default();
            TrashEntry {
                parent_dir: m.path.clone(),
                obj_name: m.obj_name.clone(),
                deleted_time: timestamp_rfc3339(m.delete_time),
                deleted_time_ts: m.delete_time,
                commit_id: m.commit_id.clone(),
                is_dir: m.obj_type == "dir",
                size: m.size,
                obj_id: m.obj_id.clone(),
                repo_id: m.repo_id.clone(),
                repo_name,
            }
        })
        .collect()
}

// ── Page-based trash listing ──────────────────────────────────────────

/// Page-based trash listing for the `/trash2/` endpoint.
pub async fn list_trash2(
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

    let items = map_trash_rows_to_entries(&rows, None);

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

    let condition = build_trash_condition(
        Condition::all().add(file_trash::Column::RepoId.eq(repo_id.to_owned())),
        query,
        time_from,
        time_to,
        suffixes,
        op_users,
    );

    let total_count = file_trash::Entity::find()
        .filter(condition.clone())
        .count(db)
        .await? as i64;

    let rows = file_trash::Entity::find()
        .filter(condition)
        .order_by(file_trash::Column::DeleteTime, Order::Desc)
        .limit(per_page as u64)
        .offset(offset)
        .all(db)
        .await?;

    let items = map_trash_rows_to_entries(&rows, None);

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
            deleted_time: timestamp_rfc3339(m.delete_time),
            deleted_time_ts: m.delete_time,
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

    // Resolve the repo + head commit once (shared by every item) instead of
    // re-fetching them inside `path_exists_in_tree` / `name_exists_in_parent`
    // and the main resolve step for each item.
    let head_root_id: Option<String> = {
        let repo_record = repos
            .repo
            .find_by_id(repo_id)
            .await?
            .ok_or_else(|| AppError::NotFound("repo not found".into()))?;
        match repo_record.head_commit_id {
            Some(cid) => {
                let head = repos
                    .commit
                    .find_by_repo_and_commit_id(repo_id, &cid)
                    .await?
                    .ok_or_else(|| AppError::NotFound("head commit not found".into()))?;
                Some(head.root_id)
            }
            None => None,
        }
    };

    // Phase A: flatten `commit_id -> paths` into an ordered item list.
    struct Item {
        commit_id: String,
        full_path: String,
        parent_dir: String,
        obj_name: String,
    }
    let mut items: Vec<Item> = Vec::new();
    for (commit_id, paths) in &restore_map {
        for full_path in paths {
            let full_path = full_path.trim_end_matches('/');
            let (parent_dir, obj_name) = match full_path.rsplit_once('/') {
                Some(("", name)) => ("/", name),
                Some((parent, name)) => (parent, name),
                None => ("/", full_path),
            };
            items.push(Item {
                commit_id: commit_id.clone(),
                full_path: full_path.to_string(),
                parent_dir: parent_dir.to_string(),
                obj_name: obj_name.to_string(),
            });
        }
    }

    // Phase B: batch-load trash records per commit, indexed by
    // (commit_id, parent_dir, obj_name).
    let mut commit_ids: Vec<String> = items.iter().map(|i| i.commit_id.clone()).collect();
    commit_ids.sort();
    commit_ids.dedup();
    let mut trash_by_key: HashMap<(String, String, String), file_trash::Model> = HashMap::new();
    for cid in &commit_ids {
        for m in repos
            .file_trash
            .find_by_repo_and_commit_id(repo_id, cid)
            .await?
        {
            trash_by_key.insert((m.commit_id.clone(), m.path.clone(), m.obj_name.clone()), m);
        }
    }

    // Phase C: batch-check which fs_objects exist (EMPTY_SHA1 is always
    // treated as existing and skipped from the query).
    let mut obj_ids: Vec<String> = Vec::new();
    for item in &items {
        if let Some(m) = trash_by_key.get(&(
            item.commit_id.clone(),
            item.parent_dir.clone(),
            item.obj_name.clone(),
        )) && m.obj_id != infra::common::EMPTY_SHA1
        {
            obj_ids.push(m.obj_id.clone());
        }
    }
    obj_ids.sort();
    obj_ids.dedup();
    let existing_fs_ids = repos
        .fs_object
        .find_existing_fs_ids(repo_id, &obj_ids)
        .await?;

    // Phase D: batch-resolve unique non-"/" parent dirs, then batch-read them
    // for the name-collision check.
    let mut parent_dirs: Vec<String> = items
        .iter()
        .map(|i| i.parent_dir.clone())
        .filter(|p| p != "/")
        .collect();
    parent_dirs.sort();
    parent_dirs.dedup();
    let mut parent_fs_id_map: HashMap<String, Option<String>> = HashMap::new();
    if let Some(root) = &head_root_id {
        let targets: Vec<(String, String)> = parent_dirs
            .iter()
            .map(|p| (root.clone(), p.clone()))
            .collect();
        let resolved = crate::fs::core::resolve_fs_ids_batch(repos, repo_id, &targets).await?;
        for (p, r) in parent_dirs.iter().zip(resolved) {
            parent_fs_id_map.insert(p.clone(), r);
        }
    }

    let mut parent_ids: Vec<String> = parent_fs_id_map
        .values()
        .filter_map(|v| v.clone())
        .filter(|id| id != infra::common::EMPTY_SHA1)
        .collect();
    parent_ids.sort();
    parent_ids.dedup();
    let dir_map = crate::fs::core::read_fs_dir_data_batch(repos, repo_id, &parent_ids).await?;

    // Phase E: sequential commit loop (stateful — each item creates a new
    // commit, so this cannot be batched).
    let mut successful_ids: Vec<i32> = Vec::new();
    for item in &items {
        let Some(model) = trash_by_key.get(&(
            item.commit_id.clone(),
            item.parent_dir.clone(),
            item.obj_name.clone(),
        )) else {
            failed.push(RevertFailedItem {
                commit_id: item.commit_id.clone(),
                path: item.full_path.clone(),
                error_msg: format!("Dirent {} not found.", item.full_path),
            });
            continue;
        };

        let trash_id = model.id;
        let obj_type = model.obj_type.clone();
        let obj_id = model.obj_id.clone();
        let is_dir = obj_type == "dir";

        if obj_id != infra::common::EMPTY_SHA1 && !existing_fs_ids.contains(&obj_id) {
            failed.push(RevertFailedItem {
                commit_id: item.commit_id.clone(),
                path: item.full_path.clone(),
                error_msg: "Object not found.".into(),
            });
            continue;
        }

        let parent_fs_id = if item.parent_dir == "/" {
            match &head_root_id {
                Some(root) => root.clone(),
                None => {
                    failed.push(RevertFailedItem {
                        commit_id: item.commit_id.clone(),
                        path: item.full_path.clone(),
                        error_msg: "No commits.".into(),
                    });
                    continue;
                }
            }
        } else {
            match parent_fs_id_map.get(&item.parent_dir) {
                Some(Some(id)) => id.clone(),
                _ => {
                    failed.push(RevertFailedItem {
                        commit_id: item.commit_id.clone(),
                        path: item.full_path.clone(),
                        error_msg: format!("Directory {} not found.", item.parent_dir),
                    });
                    continue;
                }
            }
        };

        if parent_fs_id == infra::common::EMPTY_SHA1 {
            failed.push(RevertFailedItem {
                commit_id: item.commit_id.clone(),
                path: item.full_path.clone(),
                error_msg: format!("Directory {} not found.", item.parent_dir),
            });
            continue;
        }

        if let Some(dir_data) = dir_map.get(&parent_fs_id)
            && dir_data.dirents.iter().any(|d| d.name == item.obj_name)
        {
            failed.push(RevertFailedItem {
                commit_id: item.commit_id.clone(),
                path: item.full_path.clone(),
                error_msg: "A file with the same name already exists.".into(),
            });
            continue;
        }

        let now = chrono::Utc::now().timestamp();
        let entry_size = model.size;
        let description = format!("Recovered {}", item.obj_name);

        let result = FileOps::update_dir_tree_and_commit(
            db,
            repos,
            repo_id,
            &item.parent_dir,
            &parent_fs_id,
            modifier,
            &description,
            crate::fs::core::file_ops::EMPTY_ANCESTOR_CHAIN,
            |dirents| {
                dirents.push(DirEntryData {
                    id: obj_id.clone(),
                    mode: if is_dir {
                        S_IFDIR
                    } else {
                        infra::serialization::S_IFREG
                    },
                    modifier: modifier.to_string(),
                    mtime: now,
                    name: item.obj_name.clone(),
                    size: entry_size,
                });
                Ok(())
            },
        )
        .await
        .map_err(|e| AppError::Internal(format!("Restore commit failed: {e}")));

        match result {
            Ok(_) => {
                successful_ids.push(trash_id);

                // Log activity
                activity_log::log_activity(
                    db,
                    repo_id,
                    "recover",
                    if is_dir { "dir" } else { "file" },
                    &item.full_path,
                    user_id,
                    None,
                    Some(entry_size),
                    Some(&obj_id),
                    None,
                    None,
                )
                .await;

                success.push(RevertSuccessItem {
                    path: item.full_path.clone(),
                    is_dir,
                });
            }
            Err(e) => {
                failed.push(RevertFailedItem {
                    commit_id: item.commit_id.clone(),
                    path: item.full_path.clone(),
                    error_msg: format!("Restore failed: {e}"),
                });
            }
        }
    }

    // Phase F: batch-delete the trash records of successfully restored items.
    if !successful_ids.is_empty() {
        delete_trash_records(repos, &successful_ids).await?;
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

/// Collect distinct repo_ids accessible by a user.
async fn gather_user_repo_ids(repos: &Repositories, user_id: i32) -> Result<Vec<String>, AppError> {
    let member_repos = repos.member.find_by_user_id(user_id).await?;
    let ids: Vec<String> = member_repos
        .into_iter()
        .map(|m| m.repo_id)
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect();
    Ok(ids)
}

/// Build a repo_id → repo_name lookup map.
async fn build_repo_name_lookup(
    repos: &Repositories,
    repo_ids: &[String],
) -> Result<HashMap<String, String>, AppError> {
    // One batched IN query instead of one query per repo (P3-4).
    let mut names = HashMap::new();
    for r in repos.repo.find_by_ids(repo_ids).await? {
        names.insert(r.id.clone(), r.name);
    }
    Ok(names)
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

    let repo_ids = gather_user_repo_ids(repos, user_id).await?;

    if repo_ids.is_empty() {
        return Ok(TrashListResult {
            items: Vec::new(),
            total_count: 0,
            can_search: true,
        });
    }

    let total_count = file_trash::Entity::find()
        .filter(file_trash::Column::RepoId.is_in(repo_ids.clone()))
        .count(db)
        .await? as i64;

    let repo_names = build_repo_name_lookup(repos, &repo_ids).await?;

    let rows = file_trash::Entity::find()
        .filter(file_trash::Column::RepoId.is_in(repo_ids))
        .order_by(file_trash::Column::DeleteTime, Order::Desc)
        .limit(per_page as u64)
        .offset(offset)
        .all(db)
        .await?;

    let items = map_trash_rows_to_entries(&rows, Some(&repo_names));

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

    let repo_ids = gather_user_repo_ids(repos, user_id).await?;

    if repo_ids.is_empty() {
        return Ok(TrashListResult {
            items: Vec::new(),
            total_count: 0,
            can_search: true,
        });
    }

    let condition = build_trash_condition(
        Condition::all().add(file_trash::Column::RepoId.is_in(repo_ids.clone())),
        query,
        time_from,
        time_to,
        suffixes,
        None,
    );

    let total_count = file_trash::Entity::find()
        .filter(condition.clone())
        .count(db)
        .await? as i64;

    let repo_names = build_repo_name_lookup(repos, &repo_ids).await?;

    let rows = file_trash::Entity::find()
        .filter(condition)
        .order_by(file_trash::Column::DeleteTime, Order::Desc)
        .limit(per_page as u64)
        .offset(offset)
        .all(db)
        .await?;

    let items = map_trash_rows_to_entries(&rows, Some(&repo_names));

    Ok(TrashListResult {
        items,
        total_count,
        can_search: true,
    })
}

// ─── Repo-level trash ─────────────────────────────────────────────────

/// Add a deleted repo to the trash table.
pub async fn add_deleted_repo(
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
                history_limit: Set(0),
                history_ttl_days: Set(0),
                r#type: Set("repo".to_string()),
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
