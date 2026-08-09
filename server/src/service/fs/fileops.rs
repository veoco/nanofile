use std::sync::Arc;

use sea_orm::DatabaseConnection;

use crate::fs::core::file_ops::FileOps;
use crate::repository::Repositories;
use crate::service::sync::spawn_reindex;
use base::common::DirEntryData;
use base::error::AppError;
use infra::activity_log;
use infra::common::util::{generate_unique_filename, join_path};
use infra::serialization::S_IFDIR;

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
    block_store: infra::storage::DynBlockStorage,
    indexer: Option<crate::indexer::TextIndexer>,
}

impl FileOpsService {
    pub fn new(
        db: Arc<DatabaseConnection>,
        repos: Arc<Repositories>,
        block_store: infra::storage::DynBlockStorage,
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

        // Resolve the parent dir and its ancestor chain once, so the tree
        // update below walks ancestors in O(d) instead of re-resolving every
        // level from the root (O(d²)). NotFound propagates as 404.
        let (parent_fs_id, ancestor_chain) =
            FileOps::resolve_fs_id_chain(&self.repos, repo_id, parent_dir).await?;

        let parent_data = crate::fs::core::read_fs_dir_data(&self.repos, repo_id, &parent_fs_id)
            .await
            .map_err(|e| AppError::Internal(format!("read parent dir failed: {e}")))?;

        // Sum sizes from the parent dirents instead of re-resolving each path
        // (files are O(1); directories walk their subtree once).
        let mut total_deleted: i64 = 0;
        for entry in file_names
            .iter()
            .filter_map(|name| parent_data.dirents.iter().find(|d| d.name == *name))
        {
            if entry.mode & S_IFDIR != 0 {
                total_deleted +=
                    crate::fs::core::compute_tree_size(&self.repos, repo_id, &entry.id).await?;
            } else {
                total_deleted += entry.size;
            }
        }

        let names_to_delete = file_names.to_vec();
        // O(1) membership lookup for the per-entry retain filter below,
        // instead of Vec::contains's O(n) scan per retained entry.
        let names_to_delete_set: std::collections::HashSet<&str> =
            names_to_delete.iter().map(String::as_str).collect();

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
                    let fp = join_path(parent_dir, name);
                    Some(crate::fs::core::trash::TrashItem {
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
                && let Err(e) = crate::fs::core::trash::add_batch_to_trash(
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
            &ancestor_chain,
            |dirents| {
                dirents.retain(|d| !names_to_delete_set.contains(d.name.as_str()));
                Ok(())
            },
        )
        .await
        .map_err(|e| AppError::Internal(e.to_string()))?;

        // Log activity
        for name in file_names {
            let fp = join_path(parent_dir, name);
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
                let fp = join_path(parent_dir, name);
                if let Err(e) = indexer.delete_file_async(repo_id, &fp).await {
                    tracing::warn!("Failed to delete index for {fp}: {e}");
                }
            }
        }

        crate::fs::core::adjust_repo_size(&self.repos, repo_id, -total_deleted).await?;

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

        // Resolve the source parent dir (chain unused here — the destination
        // tree update below carries the ancestor chain).
        let (src_parent_fs_id, _) =
            FileOps::resolve_fs_id_chain(&self.repos, repo_id, src_parent_dir).await?;

        let src_parent_data =
            crate::fs::core::read_fs_dir_data(&self.repos, repo_id, &src_parent_fs_id)
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

        let (dst_parent_fs_id, dst_ancestor_chain) =
            FileOps::resolve_fs_id_chain(&self.repos, repo_id, dst_dir).await?;

        let dst_parent_data =
            crate::fs::core::read_fs_dir_data(&self.repos, repo_id, &dst_parent_fs_id)
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
            &dst_ancestor_chain,
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
            let fp = join_path(dst_dir, &entry.name);
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

        // Index copied files in the background — reindexing reads the whole
        // file from block storage and must not block the batch-copy response.
        if let Some(indexer) = &self.indexer {
            for entry in &entries_to_add {
                let fp = join_path(dst_dir, &entry.name);
                spawn_reindex(
                    indexer.clone(),
                    self.block_store.clone(),
                    repo_id.to_string(),
                    fp,
                );
            }
        }

        let total_copied: i64 = entries_to_add.iter().map(|e| e.size).sum();
        crate::fs::core::adjust_repo_size(&self.repos, repo_id, total_copied).await?;

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

        // Resolve the source parent dir and its ancestor chain once, so the
        // step-1 tree update walks ancestors in O(d) instead of O(d²).
        let (src_parent_fs_id, src_ancestor_chain) =
            FileOps::resolve_fs_id_chain(&self.repos, repo_id, src_parent_dir).await?;

        let src_parent_data =
            crate::fs::core::read_fs_dir_data(&self.repos, repo_id, &src_parent_fs_id)
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

        // Pre-validate the destination exists before mutating the source tree.
        let _ = FileOps::resolve_fs_id_chain(&self.repos, repo_id, dst_dir).await?;

        // Step 1: Remove entries from source
        // O(1) membership lookup for the per-entry retain filter below,
        // instead of Vec::contains's O(n) scan per retained entry.
        let src_names_for_closure: std::collections::HashSet<&str> =
            entries_to_move.iter().map(|e| e.name.as_str()).collect();

        let intermediate_root = FileOps::update_dir_tree_no_commit(
            db,
            &self.repos,
            repo_id,
            src_parent_dir,
            &src_parent_fs_id,
            &src_ancestor_chain,
            |dirents| {
                dirents.retain(|d| !src_names_for_closure.contains(d.name.as_str()));
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
            &self.repos,
            repo_id,
            &intermediate_root,
            email,
            &remove_desc,
        )
        .await
        .map_err(|e| AppError::Internal(e.to_string()))?;

        // Step 2: Add entries to destination.
        // resolve_fs_id_chain reads the repo's latest head commit (the one
        // created by step 1), matching the previous new_head_root behaviour.
        let (new_dst_fs_id, dst_ancestor_chain) =
            FileOps::resolve_fs_id_chain(&self.repos, repo_id, dst_dir).await?;

        let new_dst_data = crate::fs::core::read_fs_dir_data(&self.repos, repo_id, &new_dst_fs_id)
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
            &dst_ancestor_chain,
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
            let old_fp = join_path(src_parent_dir, &entry.name);
            let new_fp = join_path(dst_dir, &entry.name);
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

        // Update full-text search index. Deleting the old entry is cheap and
        // stays synchronous; reindexing reads the whole file, so it runs in
        // the background.
        if let Some(indexer) = &self.indexer {
            for entry in &entries_to_move {
                let old_fp = join_path(src_parent_dir, &entry.name);
                let new_fp = join_path(dst_dir, &entry.name);
                if let Err(e) = indexer.delete_file_async(repo_id, &old_fp).await {
                    tracing::warn!("Failed to delete old index on batch move: {e}");
                }
                spawn_reindex(
                    indexer.clone(),
                    self.block_store.clone(),
                    repo_id.to_string(),
                    new_fp,
                );
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
