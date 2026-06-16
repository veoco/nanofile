use axum::{Json, extract::State};
use sea_orm::{ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter};
use serde::Deserialize;
use std::sync::Arc;

use crate::AppState;
use crate::auth::middleware::AuthUser;
use crate::entity::{commit, repo};
use crate::error::AppError;
use crate::serialization::fs_json::DirEntryData;
use crate::storage::file_ops::FileOps;

#[derive(Deserialize)]
pub struct BatchMoveRequest {
    pub src_repo_id: String,
    pub src_parent_dir: String,
    pub src_dirents: Vec<String>,
    pub dst_repo_id: String,
    pub dst_parent_dir: String,
}

/// POST /api/v2.1/repos/sync-batch-move-item/
///
/// Batch move items within a repo. Uses a two-commit approach:
/// 1. Remove from source directory, create commit
/// 2. Add to destination directory, create commit
///
/// Cross-repo moves are not supported (return 400).
pub async fn batch_move_items(
    auth: AuthUser,
    State(state): State<Arc<AppState>>,
    req: Json<BatchMoveRequest>,
) -> Result<Json<serde_json::Value>, AppError> {
    if req.src_repo_id != req.dst_repo_id {
        return Err(AppError::BadRequest("cross-repo move not supported".into()));
    }

    let repo_id = &req.src_repo_id;
    let db = state.db.as_ref();

    // Permission check
    crate::storage::check_repo_write_permission(db, repo_id, auth.user_id).await?;

    // Early return for empty move request (no items to move)
    if req.src_dirents.is_empty() {
        return Ok(Json(serde_json::json!({"success": true})));
    }

    // Get head root fs_id
    let head_root_id = get_head_root_id(db, repo_id).await?;

    // Normalize paths
    let src_dir = normalize_path(&req.src_parent_dir);
    let dst_dir = normalize_path(&req.dst_parent_dir);

    // Resolve source parent directory to find entry metadata
    let src_parent_fs_id =
        crate::storage::resolve_fs_id(db, repo_id, &head_root_id, &src_dir, None)
            .await
            .map_err(|e| AppError::Internal(format!("resolve source dir failed: {e}")))?;

    let src_parent_data = crate::storage::read_fs_dir_data(db, repo_id, &src_parent_fs_id)
        .await
        .map_err(|e| AppError::Internal(format!("read source dir failed: {e}")))?;

    // Collect source entries with their metadata
    let mut entries_to_move: Vec<DirEntryData> = Vec::new();
    let now = chrono::Utc::now().timestamp();

    for name in &req.src_dirents {
        let entry = src_parent_data
            .dirents
            .iter()
            .find(|d| d.name == *name)
            .ok_or_else(|| AppError::NotFound(format!("source file not found: {name}")))?;

        entries_to_move.push(DirEntryData {
            id: entry.id.clone(),
            mode: entry.mode,
            modifier: auth.email.clone(),
            mtime: now,
            name: entry.name.clone(),
            size: entry.size,
        });
    }

    // Resolve destination parent directory from current tree (validates it exists)
    let _dst_parent_fs_id =
        crate::storage::resolve_fs_id(db, repo_id, &head_root_id, &dst_dir, None)
            .await
            .map_err(|e| AppError::Internal(format!("resolve dest dir failed: {e}")))?;

    // Step 1: Remove entries from source parent, create commit
    let src_names_for_closure: Vec<String> =
        entries_to_move.iter().map(|e| e.name.clone()).collect();

    let intermediate_root = FileOps::update_dir_tree_no_commit(
        db,
        repo_id,
        &src_dir,
        &src_parent_fs_id,
        Some(state.path_cache.as_ref()),
        crate::storage::file_ops::EMPTY_ANCESTOR_CHAIN,
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
        repo_id,
        &intermediate_root,
        &auth.email,
        &remove_desc,
        Some(state.path_cache.as_ref()),
    )
    .await
    .map_err(|e| AppError::Internal(e.to_string()))?;

    // Step 2: Re-read head, resolve destination again, add entries with commit
    let new_head_root = get_head_root_id(db, repo_id).await?;

    let new_dst_fs_id = crate::storage::resolve_fs_id(db, repo_id, &new_head_root, &dst_dir, None)
        .await
        .map_err(|e| AppError::Internal(format!("resolve dest dir after removal failed: {e}")))?;

    FileOps::update_dir_tree_and_commit(
        db,
        repo_id,
        &dst_dir,
        &new_dst_fs_id,
        &auth.email,
        &remove_desc,
        Some(state.path_cache.as_ref()),
        crate::storage::file_ops::EMPTY_ANCESTOR_CHAIN,
        |dirents| {
            for entry in &entries_to_move {
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

    // Update full-text search index for each moved item.
    if let Some(indexer) = &state.indexer {
        for entry in &entries_to_move {
            let old_fp = if src_dir == "/" {
                format!("/{}", entry.name)
            } else {
                format!("{src_dir}/{}", entry.name)
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
                .reindex_file(db, repo_id, &new_fp, &state.block_store)
                .await
            {
                tracing::warn!("Failed to reindex on batch move: {e}");
            }
        }
    }

    Ok(Json(serde_json::json!({"success": true})))
}

#[derive(Deserialize)]
pub struct SyncBatchCopyRequest {
    pub src_repo_id: String,
    pub src_parent_dir: String,
    pub src_dirents: Vec<String>,
    pub dst_repo_id: String,
    pub dst_parent_dir: String,
}

/// Ensure path starts with "/" for consistent DB lookups.
pub(crate) fn normalize_path(path: &str) -> String {
    if path.is_empty() || path == "/" {
        "/".to_string()
    } else if path.starts_with('/') {
        path.to_string()
    } else {
        format!("/{}", path)
    }
}

/// Get the root_fs_id from the repo's head commit for path resolution.
pub(crate) async fn get_head_root_id(
    db: &DatabaseConnection,
    repo_id: &str,
) -> Result<String, AppError> {
    let repo_record = repo::Entity::find_by_id(repo_id)
        .one(db)
        .await?
        .ok_or_else(|| AppError::NotFound("Repository not found".to_string()))?;
    let head_commit_id = repo_record
        .head_commit_id
        .ok_or_else(|| AppError::NotFound("No commits yet".to_string()))?;
    let head = commit::Entity::find()
        .filter(commit::Column::RepoId.eq(repo_id))
        .filter(commit::Column::CommitId.eq(&head_commit_id))
        .one(db)
        .await?
        .ok_or_else(|| AppError::NotFound("Head commit not found".to_string()))?;
    Ok(head.root_id)
}

/// Generate a unique filename when there's a name collision.
/// Appends " (N)" before the extension, e.g. "file (1).txt", "file (2).txt".
pub(crate) fn generate_unique_filename(existing: &[DirEntryData], name: &str) -> String {
    let base = if let Some(dot) = name.rfind('.') {
        let (stem, ext) = name.split_at(dot);
        (stem.to_string(), ext.to_string())
    } else {
        (name.to_string(), String::new())
    };

    let mut i = 1;
    loop {
        let candidate = if base.1.is_empty() {
            format!("{} ({})", base.0, i)
        } else {
            format!("{} ({}){}", base.0, i, base.1)
        };
        if !existing.iter().any(|d| d.name == candidate) {
            return candidate;
        }
        i += 1;
    }
}

/// POST /api/v2.1/repos/sync-batch-copy-item/
///
/// Handles same-repo file/directory copy for the desktop client's
/// cloud file browser copy-and-paste operation.
///
/// For same-repo copies, the fs_objects are content-addressed so we
/// just duplicate the dirent entries (no block content copying needed).
pub async fn sync_batch_copy_item(
    auth: AuthUser,
    State(state): State<Arc<AppState>>,
    Json(body): Json<SyncBatchCopyRequest>,
) -> Result<Json<serde_json::Value>, AppError> {
    // Only same-repo copy is supported currently.
    if body.src_repo_id != body.dst_repo_id {
        return Err(AppError::BadRequest("cross-repo copy not supported".into()));
    }

    let repo_id = &body.src_repo_id;
    let db = state.db.as_ref();

    // Permission check
    crate::storage::check_repo_write_permission(db, repo_id, auth.user_id).await?;

    // Get head root fs_id
    let head_root_id = get_head_root_id(db, repo_id).await?;

    // Normalize paths
    let src_parent_dir = normalize_path(&body.src_parent_dir);
    let dst_parent_dir = normalize_path(&body.dst_parent_dir);

    // Resolve source parent directory to get source dirents metadata
    let src_parent_fs_id =
        crate::storage::resolve_fs_id(db, repo_id, &head_root_id, &src_parent_dir, None)
            .await
            .map_err(|e| AppError::Internal(format!("resolve source dir failed: {e}")))?;

    let src_parent_data = crate::storage::read_fs_dir_data(db, repo_id, &src_parent_fs_id)
        .await
        .map_err(|e| AppError::Internal(format!("read source dir failed: {e}")))?;

    // Collect source entries with their metadata
    let mut new_entries: Vec<DirEntryData> = Vec::new();
    let now = chrono::Utc::now().timestamp();

    for name in &body.src_dirents {
        let entry = src_parent_data
            .dirents
            .iter()
            .find(|d| d.name == *name)
            .ok_or_else(|| AppError::NotFound(format!("source file not found: {name}")))?;

        new_entries.push(DirEntryData {
            id: entry.id.clone(),
            mode: entry.mode,
            modifier: auth.email.clone(),
            mtime: now,
            name: entry.name.clone(),
            size: entry.size,
        });
    }

    // Resolve destination parent directory
    let dst_parent_fs_id =
        crate::storage::resolve_fs_id(db, repo_id, &head_root_id, &dst_parent_dir, None)
            .await
            .map_err(|e| AppError::Internal(format!("resolve dest dir failed: {e}")))?;

    // Add entries to destination parent's FsDirData and create a commit
    let description = if new_entries.len() == 1 {
        format!("Added \"{}\"", new_entries[0].name)
    } else {
        format!(
            "Added \"{}\" and {} more files",
            new_entries[0].name,
            new_entries.len() - 1
        )
    };

    FileOps::update_dir_tree_and_commit(
        db,
        repo_id,
        &dst_parent_dir,
        &dst_parent_fs_id,
        &auth.email,
        &description,
        Some(state.path_cache.as_ref()),
        crate::storage::file_ops::EMPTY_ANCESTOR_CHAIN,
        |dirents| {
            for entry in &new_entries {
                // Check for name collision and generate unique name if needed
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

    // Index copied files in full-text search.
    if let Some(indexer) = &state.indexer {
        for entry in &new_entries {
            let fp = if dst_parent_dir == "/" {
                format!("/{}", entry.name)
            } else {
                format!("{}/{}", dst_parent_dir, entry.name)
            };
            if let Err(e) = indexer
                .reindex_file(db, repo_id, &fp, &state.block_store)
                .await
            {
                tracing::warn!("Failed to index copied file {}: {e}", entry.name);
            }
        }
    }

    // Adjust repo size (add sizes of copied files).
    let total_copied: i64 = new_entries.iter().map(|e| e.size).sum();
    crate::storage::adjust_repo_size(db, repo_id, total_copied).await?;

    Ok(Json(serde_json::json!({"success": true})))
}

#[derive(Deserialize)]
pub struct BatchDeleteRequest {
    pub repo_id: String,
    pub parent_dir: String,
    pub dirents: Vec<String>,
}

/// POST /api/v2.1/repos/batch-delete-item/
///
/// Batch delete multiple files/directories from a parent directory.
/// Creates a single commit for all deletions.
pub async fn batch_delete_item(
    auth: AuthUser,
    State(state): State<Arc<AppState>>,
    Json(body): Json<BatchDeleteRequest>,
) -> Result<Json<serde_json::Value>, AppError> {
    if body.dirents.is_empty() {
        return Ok(Json(serde_json::json!({"success": true})));
    }

    let db = state.db.as_ref();
    let repo_id = &body.repo_id;

    // Permission check
    crate::storage::check_repo_write_permission(db, repo_id, auth.user_id).await?;

    let parent_dir = normalize_path(&body.parent_dir);

    let head_root_id = get_head_root_id(db, repo_id).await?;

    let parent_fs_id = crate::storage::resolve_fs_id(db, repo_id, &head_root_id, &parent_dir, None)
        .await
        .map_err(|e| AppError::Internal(format!("resolve parent dir failed: {e}")))?;

    let names_to_delete = body.dirents.clone();

    // Get total size of all items being deleted (for repo size adjustment).
    let mut total_deleted: i64 = 0;
    for name in &names_to_delete {
        let fp = if parent_dir == "/" {
            format!("/{name}")
        } else {
            format!("{parent_dir}/{name}")
        };
        if let Ok(sz) = crate::storage::get_entry_total_size(db, repo_id, &fp).await {
            total_deleted += sz;
        }
    }

    FileOps::update_dir_tree_and_commit(
        db,
        repo_id,
        &parent_dir,
        &parent_fs_id,
        &auth.email,
        &format!("Deleted {} items", names_to_delete.len()),
        Some(state.path_cache.as_ref()),
        crate::storage::file_ops::EMPTY_ANCESTOR_CHAIN,
        |dirents| {
            dirents.retain(|d| !names_to_delete.contains(&d.name));
            Ok(())
        },
    )
    .await
    .map_err(|e| AppError::Internal(e.to_string()))?;

    // Remove from full-text search index.
    if let Some(indexer) = &state.indexer {
        for name in &body.dirents {
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

    // Adjust repo size (subtract total deleted size).
    crate::storage::adjust_repo_size(db, repo_id, -total_deleted).await?;

    Ok(Json(serde_json::json!({"success": true})))
}
