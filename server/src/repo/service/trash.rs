use sea_orm::{ConnectionTrait, DatabaseBackend, DatabaseConnection, Statement};
use serde::Serialize;
use std::collections::HashMap;

use crate::activity_log;
use crate::entity::deleted_repo;
use crate::error::AppError;
use crate::repo::file_ops::FileOps;
use crate::repository::Repositories;
use crate::serialization::S_IFDIR;
use crate::serialization::fs_json::DirEntryData;

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

pub struct TrashService<'a> {
    pub db: &'a DatabaseConnection,
    pub repos: &'a Repositories,
}

impl<'a> TrashService<'a> {
    pub fn new(db: &'a DatabaseConnection, repos: &'a Repositories) -> Self {
        Self { db, repos }
    }
    /// Insert a single deleted file/dir into `file_trash`.
    ///
    /// `path` is the full path (`/dir/file.txt`). `parent_commit_id` is the
    /// commit **before** deletion (the parent still contains the entry).
    ///
    /// Best-effort: logs and swallows errors.
    #[allow(clippy::too_many_arguments)]
    pub async fn add_to_trash(
        db: &DatabaseConnection,
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

        db.execute(Statement::from_sql_and_values(
            DatabaseBackend::Sqlite,
            "INSERT INTO file_trash (user, obj_type, obj_id, obj_name, delete_time, \
             repo_id, commit_id, path, size) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)",
            vec![
                user_email.to_owned().into(),
                obj_type.to_owned().into(),
                obj_id.to_owned().into(),
                obj_name.to_owned().into(),
                now.into(),
                repo_id.to_owned().into(),
                parent_commit_id.to_owned().into(),
                parent_dir.to_owned().into(),
                size.into(),
            ],
        ))
        .await?;

        Ok(())
    }

    /// Insert multiple deleted items into `file_trash` in a single batch.
    ///
    /// Best-effort: logs and swallows errors.
    pub async fn add_batch_to_trash(
        db: &DatabaseConnection,
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

            db.execute(Statement::from_sql_and_values(
                DatabaseBackend::Sqlite,
                "INSERT INTO file_trash (user, obj_type, obj_id, obj_name, delete_time, \
                 repo_id, commit_id, path, size) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)",
                vec![
                    user_email.to_owned().into(),
                    item.obj_type.to_owned().into(),
                    item.obj_id.to_owned().into(),
                    item.obj_name.to_owned().into(),
                    now.into(),
                    repo_id.to_owned().into(),
                    parent_commit_id.to_owned().into(),
                    parent_dir.to_owned().into(),
                    item.size.into(),
                ],
            ))
            .await?;
        }

        Ok(())
    }

    /// Page-based trash listing for the `/trash2/` endpoint.
    pub async fn list_trash2(
        db: &DatabaseConnection,
        repo_id: &str,
        page: u32,
        per_page: u32,
    ) -> Result<TrashListResult, AppError> {
        let page = page.max(1);
        let per_page = per_page.clamp(1, 100);
        let offset = ((page - 1) * per_page) as i64;

        // Count total
        let count_result = db
            .query_one(Statement::from_sql_and_values(
                DatabaseBackend::Sqlite,
                "SELECT COUNT(*) FROM file_trash WHERE repo_id = $1",
                vec![repo_id.to_owned().into()],
            ))
            .await?
            .ok_or_else(|| AppError::Internal("count failed".into()))?;
        let total_count: i64 = count_result.try_get("", "").unwrap_or(0);

        // Fetch items
        let rows = db
            .query_all(Statement::from_sql_and_values(
                DatabaseBackend::Sqlite,
                "SELECT user, obj_type, obj_id, obj_name, delete_time, \
                 repo_id, commit_id, path, size \
                 FROM file_trash WHERE repo_id = $1 \
                 ORDER BY delete_time DESC \
                 LIMIT $2 OFFSET $3",
                vec![repo_id.to_owned().into(), per_page.into(), offset.into()],
            ))
            .await?;

        let items = rows
            .iter()
            .map(|r| {
                let delete_time: i64 = r.try_get("", "delete_time").unwrap_or(0);
                let obj_type: String = r.try_get("", "obj_type").unwrap_or_default();
                TrashEntry {
                    parent_dir: r.try_get("", "path").unwrap_or_default(),
                    obj_name: r.try_get("", "obj_name").unwrap_or_default(),
                    deleted_time: chrono::DateTime::from_timestamp(delete_time, 0)
                        .map(|d| d.to_rfc3339())
                        .unwrap_or_default(),
                    commit_id: r.try_get("", "commit_id").unwrap_or_default(),
                    is_dir: obj_type == "dir",
                    size: r.try_get("", "size").unwrap_or(0),
                    obj_id: r.try_get("", "obj_id").unwrap_or_default(),
                    repo_id: repo_id.to_string(),
                    repo_name: String::new(),
                }
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
        let offset = ((page - 1) * per_page) as i64;

        let mut where_clauses = vec!["ft.repo_id = $1".to_string()];
        let mut params: Vec<sea_orm::Value> = vec![repo_id.to_owned().into()];
        let mut param_idx = 2u32;

        // Keyword search on obj_name
        if !query.is_empty() {
            where_clauses.push(format!("ft.obj_name LIKE ${}", param_idx));
            params.push(format!("%{}%", query).into());
            param_idx += 1;
        }

        // Filter by users
        if let Some(users) = op_users
            && !users.is_empty()
        {
            let emails: Vec<&str> = users
                .split(',')
                .map(|s| s.trim())
                .filter(|s| !s.is_empty())
                .collect();
            if !emails.is_empty() {
                let placeholders: Vec<String> = emails
                    .iter()
                    .enumerate()
                    .map(|(i, _)| format!("${}", param_idx + i as u32))
                    .collect();
                where_clauses.push(format!("ft.user IN ({})", placeholders.join(",")));
                for email in &emails {
                    params.push((*email).to_owned().into());
                }
                param_idx += emails.len() as u32;
            }
        }

        // Time range filter
        if let Some(from) = time_from {
            where_clauses.push(format!("ft.delete_time >= ${}", param_idx));
            params.push(from.into());
            param_idx += 1;
        }
        if let Some(to) = time_to {
            where_clauses.push(format!("ft.delete_time <= ${}", param_idx));
            params.push(to.into());
            param_idx += 1;
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
                let suffix_clauses: Vec<String> = exts
                    .iter()
                    .enumerate()
                    .map(|(i, _)| format!("ft.obj_name LIKE ${}", param_idx + i as u32))
                    .collect();
                where_clauses.push(format!("({})", suffix_clauses.join(" OR ")));
                for ext in &exts {
                    let pattern = if ext.starts_with('.') {
                        format!("%{}", ext)
                    } else {
                        format!("%.{}", ext)
                    };
                    params.push(pattern.into());
                }
                param_idx += exts.len() as u32;
            }
        }

        let where_sql = where_clauses.join(" AND ");

        // Count
        let count_sql = format!("SELECT COUNT(*) FROM file_trash ft WHERE {}", where_sql);
        let count_params = params.clone();
        let count_result = db
            .query_one(Statement::from_sql_and_values(
                DatabaseBackend::Sqlite,
                &count_sql,
                count_params,
            ))
            .await?
            .ok_or_else(|| AppError::Internal("count failed".into()))?;
        let total_count: i64 = count_result.try_get("", "").unwrap_or(0);

        // Fetch items
        let select_sql = format!(
            "SELECT ft.user, ft.obj_type, ft.obj_id, ft.obj_name, ft.delete_time, \
             ft.repo_id, ft.commit_id, ft.path, ft.size \
             FROM file_trash ft WHERE {} \
             ORDER BY ft.delete_time DESC \
             LIMIT ${} OFFSET ${}",
            where_sql,
            param_idx,
            param_idx + 1,
        );
        params.push(per_page.into());
        params.push(offset.into());

        let rows = db
            .query_all(Statement::from_sql_and_values(
                DatabaseBackend::Sqlite,
                &select_sql,
                params,
            ))
            .await?;

        let items = rows
            .iter()
            .map(|r| {
                let delete_time: i64 = r.try_get("", "delete_time").unwrap_or(0);
                let obj_type: String = r.try_get("", "obj_type").unwrap_or_default();
                TrashEntry {
                    parent_dir: r.try_get("", "path").unwrap_or_default(),
                    obj_name: r.try_get("", "obj_name").unwrap_or_default(),
                    deleted_time: chrono::DateTime::from_timestamp(delete_time, 0)
                        .map(|d| d.to_rfc3339())
                        .unwrap_or_default(),
                    commit_id: r.try_get("", "commit_id").unwrap_or_default(),
                    is_dir: obj_type == "dir",
                    size: r.try_get("", "size").unwrap_or(0),
                    obj_id: r.try_get("", "obj_id").unwrap_or_default(),
                    repo_id: repo_id.to_string(),
                    repo_name: String::new(),
                }
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
        db: &DatabaseConnection,
        repo_id: &str,
        cursor: Option<i64>,
        limit: u32,
    ) -> Result<CursorTrashResult, AppError> {
        let limit = limit.clamp(1, 100);
        let fetch = limit + 1; // fetch one extra to detect has_more

        let (where_clause, mut params) = if let Some(c) = cursor {
            (
                "WHERE repo_id = $1 AND delete_time < $2".to_string(),
                vec![repo_id.to_owned().into(), c.into()] as Vec<sea_orm::Value>,
            )
        } else {
            (
                "WHERE repo_id = $1".to_string(),
                vec![repo_id.to_owned().into()] as Vec<sea_orm::Value>,
            )
        };

        let limit_param = format!("${}", params.len() + 1);
        params.push(fetch.into());

        let sql = format!(
            "SELECT user, obj_type, obj_id, obj_name, delete_time, \
             repo_id, commit_id, path, size \
             FROM file_trash {} \
             ORDER BY delete_time DESC \
             LIMIT {}",
            where_clause, limit_param,
        );

        let rows = db
            .query_all(Statement::from_sql_and_values(
                DatabaseBackend::Sqlite,
                &sql,
                params,
            ))
            .await?;

        let has_more = rows.len() > limit as usize;
        let items: Vec<TrashEntry> = rows
            .into_iter()
            .take(limit as usize)
            .map(|r| {
                let delete_time: i64 = r.try_get("", "delete_time").unwrap_or(0);
                let obj_type: String = r.try_get("", "obj_type").unwrap_or_default();
                TrashEntry {
                    parent_dir: r.try_get("", "path").unwrap_or_default(),
                    obj_name: r.try_get("", "obj_name").unwrap_or_default(),
                    deleted_time: chrono::DateTime::from_timestamp(delete_time, 0)
                        .map(|d| d.to_rfc3339())
                        .unwrap_or_default(),
                    commit_id: r.try_get("", "commit_id").unwrap_or_default(),
                    is_dir: obj_type == "dir",
                    size: r.try_get("", "size").unwrap_or(0),
                    obj_id: r.try_get("", "obj_id").unwrap_or_default(),
                    repo_id: repo_id.to_string(),
                    repo_name: String::new(),
                }
            })
            .collect();

        Ok(CursorTrashResult { items, has_more })
    }

    /// Remove trash records by their primary key IDs.
    async fn delete_trash_records(db: &DatabaseConnection, ids: &[i32]) -> Result<(), AppError> {
        for chunk in ids.chunks(100) {
            let placeholders: Vec<String> = chunk
                .iter()
                .enumerate()
                .map(|(i, _)| format!("${}", i + 1))
                .collect();
            let sql = format!(
                "DELETE FROM file_trash WHERE id IN ({})",
                placeholders.join(",")
            );
            let params: Vec<sea_orm::Value> = chunk.iter().map(|id| (*id).into()).collect();
            db.execute(Statement::from_sql_and_values(
                DatabaseBackend::Sqlite,
                &sql,
                params,
            ))
            .await?;
        }
        Ok(())
    }

    /// Check whether a path exists in the current FS tree (i.e. the resolved
    /// fs_id from the repo's head commit actually exists).
    async fn path_exists_in_tree(
        db: &DatabaseConnection,
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

        match crate::repo::resolve_fs_id(db, repo_id, &head.root_id, path).await {
            Ok(_) => Ok(true),
            Err(_) => Ok(false),
        }
    }

    /// Check whether a file/dir name already exists in a parent directory in
    /// the current FS tree.
    async fn name_exists_in_parent(
        db: &DatabaseConnection,
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
            match crate::repo::resolve_fs_id(db, repo_id, &head.root_id, parent_path).await {
                Ok(id) => id,
                Err(_) => return Ok(false),
            };

        // Read parent directory entries
        let parent_data = match crate::repo::read_fs_dir_data(db, repo_id, &parent_fs_id).await {
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
                let rows = db
                    .query_all(Statement::from_sql_and_values(
                        DatabaseBackend::Sqlite,
                        "SELECT id, obj_type, obj_id, obj_name, path, size \
                         FROM file_trash \
                         WHERE repo_id = $1 AND commit_id = $2 AND path = $3 AND obj_name = $4",
                        vec![
                            repo_id.to_owned().into(),
                            commit_id.to_owned().into(),
                            parent_dir.to_owned().into(),
                            obj_name.to_owned().into(),
                        ],
                    ))
                    .await?;

                let Some(row) = rows.first() else {
                    failed.push(RevertFailedItem {
                        commit_id: commit_id.clone(),
                        path: full_path.to_string(),
                        error_msg: format!("Dirent {full_path} not found."),
                    });
                    continue;
                };

                let trash_id: i32 = row.try_get("", "id").unwrap_or(0);
                let obj_type: String = row.try_get("", "obj_type").unwrap_or_default();
                let obj_id: String = row.try_get("", "obj_id").unwrap_or_default();
                let is_dir = obj_type == "dir";

                // Verify fs_object exists
                if !Self::fs_object_exists(repos, repo_id, &obj_id).await? {
                    failed.push(RevertFailedItem {
                        commit_id: commit_id.clone(),
                        path: full_path.to_string(),
                        error_msg: "Object not found.".into(),
                    });
                    continue;
                }

                // Verify parent directory exists in current tree
                if parent_dir != "/"
                    && !Self::path_exists_in_tree(db, repos, repo_id, parent_dir).await?
                {
                    failed.push(RevertFailedItem {
                        commit_id: commit_id.clone(),
                        path: full_path.to_string(),
                        error_msg: format!("Directory {parent_dir} not found."),
                    });
                    continue;
                }

                // Check for name collision
                if Self::name_exists_in_parent(db, repos, repo_id, parent_dir, obj_name).await? {
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
                    match crate::repo::resolve_fs_id(db, repo_id, &head.root_id, parent_dir).await {
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
                let entry_size: i64 = row.try_get("", "size").unwrap_or(0);
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
                        Self::delete_trash_records(db, &[trash_id]).await?;

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
        Self::restore_trash_items(db, repos, repo_id, modifier, user_id, map).await
    }

    /// Clean trash items for a repo, optionally keeping items newer than
    /// `keep_days`. Returns the number of deleted rows.
    pub async fn clean_trash(
        db: &DatabaseConnection,
        repo_id: &str,
        keep_days: Option<i64>,
    ) -> Result<u64, AppError> {
        let cutoff = keep_days
            .filter(|d| *d > 0)
            .map(|d| chrono::Utc::now().timestamp() - d * 86400);

        let result = if let Some(c) = cutoff {
            db.execute(Statement::from_sql_and_values(
                DatabaseBackend::Sqlite,
                "DELETE FROM file_trash WHERE repo_id = $1 AND delete_time < $2",
                vec![repo_id.to_owned().into(), c.into()],
            ))
            .await?
        } else {
            db.execute(Statement::from_sql_and_values(
                DatabaseBackend::Sqlite,
                "DELETE FROM file_trash WHERE repo_id = $1",
                vec![repo_id.to_owned().into()],
            ))
            .await?
        };

        Ok(result.rows_affected())
    }

    /// List trash items across all repos the user has access to.
    ///
    /// Joins with `repo_members` to find accessible repos, and `repos` to
    /// include the repo name for display.
    pub async fn list_trash_for_user(
        db: &DatabaseConnection,
        user_id: i32,
        page: u32,
        per_page: u32,
    ) -> Result<TrashListResult, AppError> {
        let page = page.max(1);
        let per_page = per_page.clamp(1, 100);
        let offset = ((page - 1) * per_page) as i64;

        // Count
        let count_result = db
            .query_one(Statement::from_sql_and_values(
                DatabaseBackend::Sqlite,
                "SELECT COUNT(*) FROM file_trash ft \
                 INNER JOIN repo_members rm ON ft.repo_id = rm.repo_id AND rm.user_id = $1",
                vec![user_id.into()],
            ))
            .await?
            .ok_or_else(|| AppError::Internal("count failed".into()))?;
        let total_count: i64 = count_result.try_get("", "").unwrap_or(0);

        // Fetch items
        let rows = db
            .query_all(Statement::from_sql_and_values(
                DatabaseBackend::Sqlite,
                "SELECT ft.user, ft.obj_type, ft.obj_id, ft.obj_name, ft.delete_time, \
                 ft.repo_id, ft.commit_id, ft.path, ft.size, \
                 COALESCE(r.name, '') AS repo_name \
                 FROM file_trash ft \
                 INNER JOIN repo_members rm ON ft.repo_id = rm.repo_id AND rm.user_id = $1 \
                 LEFT JOIN repos r ON ft.repo_id = r.id \
                 ORDER BY ft.delete_time DESC \
                 LIMIT $2 OFFSET $3",
                vec![user_id.into(), per_page.into(), offset.into()],
            ))
            .await?;

        let items = rows
            .iter()
            .map(|r| {
                let delete_time: i64 = r.try_get("", "delete_time").unwrap_or(0);
                let obj_type: String = r.try_get("", "obj_type").unwrap_or_default();
                let repo_id: String = r.try_get("", "repo_id").unwrap_or_default();
                let repo_name: String = r.try_get("", "repo_name").unwrap_or_default();
                TrashEntry {
                    parent_dir: r.try_get("", "path").unwrap_or_default(),
                    obj_name: r.try_get("", "obj_name").unwrap_or_default(),
                    deleted_time: chrono::DateTime::from_timestamp(delete_time, 0)
                        .map(|d| d.to_rfc3339())
                        .unwrap_or_default(),
                    commit_id: r.try_get("", "commit_id").unwrap_or_default(),
                    is_dir: obj_type == "dir",
                    size: r.try_get("", "size").unwrap_or(0),
                    obj_id: r.try_get("", "obj_id").unwrap_or_default(),
                    repo_id,
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
        let offset = ((page - 1) * per_page) as i64;

        let mut where_clauses = vec![
            "rm.user_id = $1".to_string(),
            "rm.repo_id = ft.repo_id".to_string(),
        ];
        let mut params: Vec<sea_orm::Value> = vec![user_id.into()];
        let mut param_idx = 2u32;

        // Keyword search on obj_name
        if !query.is_empty() {
            where_clauses.push(format!("ft.obj_name LIKE ${}", param_idx));
            params.push(format!("%{}%", query).into());
            param_idx += 1;
        }

        // Time range filter
        if let Some(from) = time_from {
            where_clauses.push(format!("ft.delete_time >= ${}", param_idx));
            params.push(from.into());
            param_idx += 1;
        }
        if let Some(to) = time_to {
            where_clauses.push(format!("ft.delete_time <= ${}", param_idx));
            params.push(to.into());
            param_idx += 1;
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
                let suffix_clauses: Vec<String> = exts
                    .iter()
                    .enumerate()
                    .map(|(i, _)| format!("ft.obj_name LIKE ${}", param_idx + i as u32))
                    .collect();
                where_clauses.push(format!("({})", suffix_clauses.join(" OR ")));
                for ext in &exts {
                    let pattern = if ext.starts_with('.') {
                        format!("%{}", ext)
                    } else {
                        format!("%.{}", ext)
                    };
                    params.push(pattern.into());
                }
                param_idx += exts.len() as u32;
            }
        }

        let where_sql = where_clauses.join(" AND ");

        // Count
        let count_sql = format!(
            "SELECT COUNT(*) FROM file_trash ft, repo_members rm WHERE {}",
            where_sql
        );
        let count_params = params.clone();
        let count_result = db
            .query_one(Statement::from_sql_and_values(
                DatabaseBackend::Sqlite,
                &count_sql,
                count_params,
            ))
            .await?
            .ok_or_else(|| AppError::Internal("count failed".into()))?;
        let total_count: i64 = count_result.try_get("", "").unwrap_or(0);

        // Fetch items
        let select_sql = format!(
            "SELECT ft.user, ft.obj_type, ft.obj_id, ft.obj_name, ft.delete_time, \
             ft.repo_id, ft.commit_id, ft.path, ft.size, \
             COALESCE(r.name, '') AS repo_name \
             FROM file_trash ft, repo_members rm \
             LEFT JOIN repos r ON ft.repo_id = r.id \
             WHERE {} \
             ORDER BY ft.delete_time DESC \
             LIMIT ${} OFFSET ${}",
            where_sql,
            param_idx,
            param_idx + 1,
        );
        params.push(per_page.into());
        params.push(offset.into());

        let rows = db
            .query_all(Statement::from_sql_and_values(
                DatabaseBackend::Sqlite,
                &select_sql,
                params,
            ))
            .await?;

        let items = rows
            .iter()
            .map(|r| {
                let delete_time: i64 = r.try_get("", "delete_time").unwrap_or(0);
                let obj_type: String = r.try_get("", "obj_type").unwrap_or_default();
                let repo_id: String = r.try_get("", "repo_id").unwrap_or_default();
                let repo_name: String = r.try_get("", "repo_name").unwrap_or_default();
                TrashEntry {
                    parent_dir: r.try_get("", "path").unwrap_or_default(),
                    obj_name: r.try_get("", "obj_name").unwrap_or_default(),
                    deleted_time: chrono::DateTime::from_timestamp(delete_time, 0)
                        .map(|d| d.to_rfc3339())
                        .unwrap_or_default(),
                    commit_id: r.try_get("", "commit_id").unwrap_or_default(),
                    is_dir: obj_type == "dir",
                    size: r.try_get("", "size").unwrap_or(0),
                    obj_id: r.try_get("", "obj_id").unwrap_or_default(),
                    repo_id,
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
        db: &DatabaseConnection,
        repo_id: &str,
        repo_name: &str,
        head_id: Option<&str>,
        owner_id: i32,
        size: i64,
    ) -> Result<(), AppError> {
        let now = chrono::Utc::now().timestamp();

        db.execute(Statement::from_sql_and_values(
            DatabaseBackend::Sqlite,
            "INSERT OR IGNORE INTO deleted_repos (repo_id, repo_name, head_id, owner_id, size, del_time) \
             VALUES ($1, $2, $3, $4, $5, $6)",
            vec![
                repo_id.to_owned().into(),
                repo_name.to_owned().into(),
                head_id.map(|s| s.to_owned()).into(),
                owner_id.into(),
                size.into(),
                now.into(),
            ],
        ))
        .await?;

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

        // Re-insert repo (use original head_commit_id if available)
        // Use raw SQL to avoid ORM issues with optional head_commit_id.
        let _ = db
            .execute(Statement::from_sql_and_values(
                DatabaseBackend::Sqlite,
                "INSERT OR IGNORE INTO repos \
                 (id, name, description, owner_id, encrypted, enc_version, \
                  salt, permission, created_at, updated_at, size, repo_version) \
                 VALUES ($1, $2, $3, $4, 0, 0, '', 'rw', $5, $6, $7, 1)",
                vec![
                    trashed.repo_id.clone().into(),
                    trashed.repo_name.clone().into(),
                    String::new().into(),
                    trashed.owner_id.into(),
                    now.into(),
                    now.into(),
                    trashed.size.into(),
                ],
            ))
            .await?;

        // Re-create owner membership
        let _ = db
            .execute(Statement::from_sql_and_values(
                DatabaseBackend::Sqlite,
                "INSERT OR IGNORE INTO repo_members (repo_id, user_id, permission) \
                 VALUES ($1, $2, 'rw')",
                vec![trashed.repo_id.clone().into(), trashed.owner_id.into()],
            ))
            .await?;

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
}
