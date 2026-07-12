use std::sync::Arc;

use sea_orm::DatabaseConnection;

use crate::activity_log;
use crate::common::util::{generate_unique_filename, get_head_root_id};
use crate::error::AppError;
use crate::repo::file_ops::FileOps;
use crate::repository::Repositories;
use crate::serialization::S_IFDIR;
use base::common::DirEntryData;

/// Parse colon-separated file_names into a Vec<String>.
pub fn parse_file_names(s: &str) -> Vec<String> {
    if s.is_empty() {
        return vec![];
    }
    s.split(':')
        .filter(|n| !n.is_empty())
        .map(|n| n.to_string())
        .collect()
}

// ── FileOpsService ─────────────────────────────────────────────────────

pub struct FileOpsService {
    db: Arc<DatabaseConnection>,
    repos: Arc<Repositories>,
    block_store: crate::storage::DynBlockStorage,
    indexer: Option<crate::indexer::TextIndexer>,
}

impl FileOpsService {
    pub fn new(
        db: Arc<DatabaseConnection>,
        repos: Arc<Repositories>,
        block_store: crate::storage::DynBlockStorage,
        indexer: Option<crate::indexer::TextIndexer>,
    ) -> Self {
        Self {
            db,
            repos,
            block_store,
            indexer,
        }
    }

    fn db(&self) -> &DatabaseConnection {
        self.db.as_ref()
    }

    /// Batch delete files/directories from a parent directory.
    ///
    /// Returns an optional directory listing (if caller wants reloaddir).
    pub async fn batch_delete(
        &self,
        repo_id: &str,
        parent_dir: &str,
        file_names: &[String],
        email: &str,
        user_id: i32,
    ) -> Result<(), AppError> {
        if file_names.is_empty() {
            return Ok(());
        }

        let db = self.db();
        let head_root_id = get_head_root_id(db, repo_id).await?;

        let parent_fs_id =
            crate::repo::resolve_fs_id(&self.repos, repo_id, &head_root_id, parent_dir)
                .await
                .map_err(|e| AppError::Internal(format!("resolve parent dir failed: {e}")))?;

        let mut total_deleted: i64 = 0;
        for name in file_names {
            let fp = if parent_dir == "/" {
                format!("/{name}")
            } else {
                format!("{parent_dir}/{name}")
            };
            if let Ok(sz) = crate::repo::get_entry_total_size(db, &self.repos, repo_id, &fp).await {
                total_deleted += sz;
            }
        }

        let parent_data = crate::repo::read_fs_dir_data(&self.repos, repo_id, &parent_fs_id)
            .await
            .map_err(|e| AppError::Internal(format!("read parent dir failed: {e}")))?;

        let names_to_delete = file_names.to_vec();

        // Record trash
        let trash_head_commit_id: Option<String> = self
            .repos
            .repo
            .find_by_id(repo_id)
            .await
            .ok()
            .flatten()
            .and_then(|r| r.head_commit_id);
        if let Some(ref parent_commit_id) = trash_head_commit_id {
            let trash_items: Vec<_> = file_names
                .iter()
                .filter_map(|name| {
                    let entry = parent_data.dirents.iter().find(|d| d.name == *name)?;
                    let fp = if parent_dir == "/" {
                        format!("/{name}")
                    } else {
                        format!("{parent_dir}/{name}")
                    };
                    Some(crate::repo::trash::TrashItem {
                        path: fp,
                        obj_type: if entry.mode & S_IFDIR != 0 {
                            "dir".to_string()
                        } else {
                            "file".to_string()
                        },
                        obj_id: entry.id.clone(),
                        obj_name: entry.name.clone(),
                        size: entry.size,
                    })
                })
                .collect();
            if !trash_items.is_empty()
                && let Err(e) = crate::repo::trash::add_batch_to_trash(
                    db,
                    &self.repos,
                    repo_id,
                    trash_items,
                    parent_commit_id,
                    email,
                )
                .await
            {
                tracing::warn!("Failed to record batch trash: {e}");
            }
        }

        FileOps::update_dir_tree_and_commit(
            db,
            &self.repos,
            repo_id,
            parent_dir,
            &parent_fs_id,
            email,
            &format!("Deleted {} items", names_to_delete.len()),
            crate::repo::file_ops::EMPTY_ANCESTOR_CHAIN,
            |dirents| {
                dirents.retain(|d| !names_to_delete.contains(&d.name));
                Ok(())
            },
        )
        .await
        .map_err(|e| AppError::Internal(e.to_string()))?;

        // Log activity
        for name in file_names {
            let fp = if parent_dir == "/" {
                format!("/{name}")
            } else {
                format!("{parent_dir}/{name}")
            };
            let entry = parent_data.dirents.iter().find(|d| d.name == *name);
            let is_dir = entry.is_some_and(|d| d.mode & S_IFDIR != 0);
            activity_log::log_activity(
                db,
                repo_id,
                "delete",
                if is_dir { "dir" } else { "file" },
                &fp,
                user_id,
                None,
                entry.map(|d| d.size),
                entry.map(|d| d.id.as_str()),
                None,
                None,
            )
            .await;
        }

        // Remove from full-text search index
        if let Some(indexer) = &self.indexer {
            for name in file_names {
                let fp = if parent_dir == "/" {
                    format!("/{name}")
                } else {
                    format!("{parent_dir}/{name}")
                };
                if let Err(e) = indexer.delete_file(repo_id, &fp) {
                    tracing::warn!("Failed to delete index for {fp}: {e}");
                }
            }
        }

        crate::repo::adjust_repo_size(db, &self.repos, repo_id, -total_deleted).await?;

        Ok(())
    }

    /// Batch copy files/directories within the same repo.
    ///
    /// Returns a list of `(obj_name, parent_dir, repo_id)` results.
    pub async fn batch_copy(
        &self,
        repo_id: &str,
        src_parent_dir: &str,
        dst_dir: &str,
        file_names: &[String],
        email: &str,
        user_id: i32,
    ) -> Result<Vec<BatchOpResult>, AppError> {
        if file_names.is_empty() {
            return Ok(Vec::new());
        }

        let db = self.db();
        let head_root_id = get_head_root_id(db, repo_id).await?;

        let src_parent_fs_id =
            crate::repo::resolve_fs_id(&self.repos, repo_id, &head_root_id, src_parent_dir)
                .await
                .map_err(|e| AppError::Internal(format!("resolve source dir failed: {e}")))?;

        let src_parent_data =
            crate::repo::read_fs_dir_data(&self.repos, repo_id, &src_parent_fs_id)
                .await
                .map_err(|e| AppError::Internal(format!("read source dir failed: {e}")))?;

        let mut new_entries: Vec<DirEntryData> = Vec::new();
        let now = chrono::Utc::now().timestamp();

        for name in file_names {
            let entry = src_parent_data
                .dirents
                .iter()
                .find(|d| d.name == *name)
                .ok_or_else(|| AppError::NotFound(format!("source file not found: {name}")))?;

            new_entries.push(DirEntryData {
                id: entry.id.clone(),
                mode: entry.mode,
                modifier: email.to_string(),
                mtime: now,
                name: entry.name.clone(),
                size: entry.size,
            });
        }

        let dst_parent_fs_id =
            crate::repo::resolve_fs_id(&self.repos, repo_id, &head_root_id, dst_dir)
                .await
                .map_err(|e| AppError::Internal(format!("resolve dest dir failed: {e}")))?;

        let dst_parent_data =
            crate::repo::read_fs_dir_data(&self.repos, repo_id, &dst_parent_fs_id)
                .await
                .map_err(|e| AppError::Internal(format!("read dest dir failed: {e}")))?;

        let mut results: Vec<BatchOpResult> = Vec::new();
        let mut entries_to_add: Vec<DirEntryData> = Vec::new();

        for entry in &new_entries {
            let obj_name = if dst_parent_data.dirents.iter().any(|d| d.name == entry.name) {
                generate_unique_filename(&dst_parent_data.dirents, &entry.name)
            } else {
                entry.name.clone()
            };

            results.push(BatchOpResult {
                repo_id: repo_id.to_string(),
                parent_dir: dst_dir.to_string(),
                obj_name: obj_name.clone(),
            });

            entries_to_add.push(DirEntryData {
                name: obj_name,
                ..entry.clone()
            });
        }

        let description = if entries_to_add.len() == 1 {
            format!("Added \"{}\"", entries_to_add[0].name)
        } else {
            format!(
                "Added \"{}\" and {} more files",
                entries_to_add[0].name,
                entries_to_add.len() - 1
            )
        };

        FileOps::update_dir_tree_and_commit(
            db,
            &self.repos,
            repo_id,
            dst_dir,
            &dst_parent_fs_id,
            email,
            &description,
            crate::repo::file_ops::EMPTY_ANCESTOR_CHAIN,
            |dirents| {
                for entry in &entries_to_add {
                    if dirents.iter().any(|d| d.name == entry.name) {
                        let unique_name = generate_unique_filename(dirents, &entry.name);
                        dirents.push(DirEntryData {
                            name: unique_name,
                            ..entry.clone()
                        });
                    } else {
                        dirents.push(entry.clone());
                    }
                }
                Ok(())
            },
        )
        .await
        .map_err(|e| AppError::Internal(e.to_string()))?;

        // Log activity
        for entry in &entries_to_add {
            let fp = if dst_dir == "/" {
                format!("/{}", entry.name)
            } else {
                format!("{dst_dir}/{}", entry.name)
            };
            let obj_type = if entry.mode & S_IFDIR != 0 {
                "dir"
            } else {
                "file"
            };
            activity_log::log_activity(
                db,
                repo_id,
                "create",
                obj_type,
                &fp,
                user_id,
                None,
                Some(entry.size),
                Some(entry.id.as_str()),
                None,
                None,
            )
            .await;
        }

        // Index copied files
        if let Some(indexer) = &self.indexer {
            for entry in &entries_to_add {
                let fp = if dst_dir == "/" {
                    format!("/{}", entry.name)
                } else {
                    format!("{dst_dir}/{}", entry.name)
                };
                if let Err(e) = indexer
                    .reindex_file(db, repo_id, &fp, &self.block_store)
                    .await
                {
                    tracing::warn!("Failed to index copied file {}: {e}", entry.name);
                }
            }
        }

        let total_copied: i64 = entries_to_add.iter().map(|e| e.size).sum();
        crate::repo::adjust_repo_size(db, &self.repos, repo_id, total_copied).await?;

        Ok(results)
    }

    /// Batch move files/directories within the same repo.
    ///
    /// Uses a two-commit approach:
    /// 1. Remove from source directory, create commit
    /// 2. Add to destination directory, create commit
    ///
    /// Returns a list of `(obj_name, parent_dir, repo_id)` results.
    pub async fn batch_move(
        &self,
        repo_id: &str,
        src_parent_dir: &str,
        dst_dir: &str,
        file_names: &[String],
        email: &str,
        user_id: i32,
    ) -> Result<Vec<BatchOpResult>, AppError> {
        if file_names.is_empty() {
            return Ok(Vec::new());
        }

        let db = self.db();
        let head_root_id = get_head_root_id(db, repo_id).await?;

        let src_parent_fs_id =
            crate::repo::resolve_fs_id(&self.repos, repo_id, &head_root_id, src_parent_dir)
                .await
                .map_err(|e| AppError::Internal(format!("resolve source dir failed: {e}")))?;

        let src_parent_data =
            crate::repo::read_fs_dir_data(&self.repos, repo_id, &src_parent_fs_id)
                .await
                .map_err(|e| AppError::Internal(format!("read source dir failed: {e}")))?;

        let mut entries_to_move: Vec<DirEntryData> = Vec::new();
        let now = chrono::Utc::now().timestamp();

        for name in file_names {
            let entry = src_parent_data
                .dirents
                .iter()
                .find(|d| d.name == *name)
                .ok_or_else(|| AppError::NotFound(format!("source file not found: {name}")))?;

            entries_to_move.push(DirEntryData {
                id: entry.id.clone(),
                mode: entry.mode,
                modifier: email.to_string(),
                mtime: now,
                name: entry.name.clone(),
                size: entry.size,
            });
        }

        let _dst_parent_fs_id =
            crate::repo::resolve_fs_id(&self.repos, repo_id, &head_root_id, dst_dir)
                .await
                .map_err(|e| AppError::Internal(format!("resolve dest dir failed: {e}")))?;

        // Step 1: Remove entries from source
        let src_names_for_closure: Vec<String> =
            entries_to_move.iter().map(|e| e.name.clone()).collect();

        let intermediate_root = FileOps::update_dir_tree_no_commit(
            db,
            &self.repos,
            repo_id,
            src_parent_dir,
            &src_parent_fs_id,
            crate::repo::file_ops::EMPTY_ANCESTOR_CHAIN,
            |dirents| {
                dirents.retain(|d| !src_names_for_closure.contains(&d.name));
                Ok(())
            },
        )
        .await
        .map_err(|e| AppError::Internal(e.to_string()))?;

        let remove_desc = if entries_to_move.len() == 1 {
            format!("Moved \"{}\"", entries_to_move[0].name)
        } else {
            format!(
                "Moved \"{}\" and {} more items",
                entries_to_move[0].name,
                entries_to_move.len() - 1
            )
        };

        FileOps::create_commit(
            db,
            &self.repos,
            repo_id,
            &intermediate_root,
            email,
            &remove_desc,
        )
        .await
        .map_err(|e| AppError::Internal(e.to_string()))?;

        // Step 2: Add entries to destination
        let new_head_root = get_head_root_id(db, repo_id).await?;

        let new_dst_fs_id =
            crate::repo::resolve_fs_id(&self.repos, repo_id, &new_head_root, dst_dir)
                .await
                .map_err(|e| {
                    AppError::Internal(format!("resolve dest dir after removal failed: {e}"))
                })?;

        let new_dst_data = crate::repo::read_fs_dir_data(&self.repos, repo_id, &new_dst_fs_id)
            .await
            .map_err(|e| AppError::Internal(format!("read dest dir failed: {e}")))?;

        let mut results: Vec<BatchOpResult> = Vec::new();
        let mut entries_to_add: Vec<DirEntryData> = Vec::new();

        for entry in &entries_to_move {
            let obj_name = if new_dst_data.dirents.iter().any(|d| d.name == entry.name) {
                generate_unique_filename(&new_dst_data.dirents, &entry.name)
            } else {
                entry.name.clone()
            };

            results.push(BatchOpResult {
                repo_id: repo_id.to_string(),
                parent_dir: dst_dir.to_string(),
                obj_name: obj_name.clone(),
            });

            entries_to_add.push(DirEntryData {
                name: obj_name,
                ..entry.clone()
            });
        }

        FileOps::update_dir_tree_and_commit(
            db,
            &self.repos,
            repo_id,
            dst_dir,
            &new_dst_fs_id,
            email,
            &remove_desc,
            crate::repo::file_ops::EMPTY_ANCESTOR_CHAIN,
            |dirents| {
                for entry in &entries_to_add {
                    if dirents.iter().any(|d| d.name == entry.name) {
                        let unique_name = generate_unique_filename(dirents, &entry.name);
                        dirents.push(DirEntryData {
                            name: unique_name,
                            ..entry.clone()
                        });
                    } else {
                        dirents.push(entry.clone());
                    }
                }
                Ok(())
            },
        )
        .await
        .map_err(|e| AppError::Internal(e.to_string()))?;

        // Log activity
        for entry in &entries_to_add {
            let old_fp = if src_parent_dir == "/" {
                format!("/{}", entry.name)
            } else {
                format!("{src_parent_dir}/{}", entry.name)
            };
            let new_fp = if dst_dir == "/" {
                format!("/{}", entry.name)
            } else {
                format!("{dst_dir}/{}", entry.name)
            };
            let obj_type = if entry.mode & S_IFDIR != 0 {
                "dir"
            } else {
                "file"
            };
            activity_log::log_activity(
                db,
                repo_id,
                "move",
                obj_type,
                &new_fp,
                user_id,
                Some(&old_fp),
                Some(entry.size),
                Some(entry.id.as_str()),
                None,
                None,
            )
            .await;
        }

        // Update full-text search index
        if let Some(indexer) = &self.indexer {
            for entry in &entries_to_move {
                let old_fp = if src_parent_dir == "/" {
                    format!("/{}", entry.name)
                } else {
                    format!("{src_parent_dir}/{}", entry.name)
                };
                let new_fp = if dst_dir == "/" {
                    format!("/{}", entry.name)
                } else {
                    format!("{dst_dir}/{}", entry.name)
                };
                if let Err(e) = indexer.delete_file(repo_id, &old_fp) {
                    tracing::warn!("Failed to delete old index on batch move: {e}");
                }
                if let Err(e) = indexer
                    .reindex_file(db, repo_id, &new_fp, &self.block_store)
                    .await
                {
                    tracing::warn!("Failed to reindex on batch move: {e}");
                }
            }
        }

        Ok(results)
    }
}

/// Result of a batch copy/move operation.
#[derive(Clone)]
pub struct BatchOpResult {
    pub repo_id: String,
    pub parent_dir: String,
    pub obj_name: String,
}
