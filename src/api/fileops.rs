use axum::body::Body;
use axum::{
    Json, Router,
    extract::{Path, Query, State},
    http::{Request, StatusCode},
    response::{IntoResponse, Response},
};
use sea_orm::{ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::HashMap;
use std::sync::Arc;

use crate::AppState;
use crate::activity_log;
use crate::auth::middleware::AuthUser;
use crate::error::AppError;
use crate::serialization::S_IFDIR;
use crate::serialization::fs_json::{DirEntryData, FsDirData};
use crate::storage::file_ops::FileOps;

// ─── Helpers ─────────────────────────────────────────────────────────────────

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
    let repo_record = crate::entity::repo::Entity::find_by_id(repo_id)
        .one(db)
        .await?
        .ok_or_else(|| AppError::NotFound("Repository not found".to_string()))?;
    let head_commit_id = repo_record
        .head_commit_id
        .ok_or_else(|| AppError::NotFound("No commits yet".to_string()))?;
    let head = crate::entity::commit::Entity::find()
        .filter(crate::entity::commit::Column::RepoId.eq(repo_id))
        .filter(crate::entity::commit::Column::CommitId.eq(&head_commit_id))
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

/// Parse colon-separated file_names into a Vec<String>.
fn parse_file_names(s: &str) -> Vec<String> {
    if s.is_empty() {
        return vec![];
    }
    s.split(':')
        .filter(|n| !n.is_empty())
        .map(|n| n.to_string())
        .collect()
}

/// Read and parse an FsDirData object from the fs_objects table.
async fn read_fs_dir_data(
    db: &DatabaseConnection,
    repo_id: &str,
    fs_id: &str,
) -> Result<FsDirData, AppError> {
    crate::storage::read_fs_dir_data(db, repo_id, fs_id)
        .await
        .map_err(|e| AppError::internal(format!("read fs_dir_data failed: {e}")))
}

/// Get the directory listing for a path (reloaddir=true support).
#[derive(Serialize)]
pub struct DirEntry {
    pub id: String,
    #[serde(rename = "type")]
    pub entry_type: String,
    pub name: String,
    pub size: i64,
    pub mtime: i64,
    pub permission: String,
}

async fn list_dir_from_fs_tree(
    db: &DatabaseConnection,
    repo_id: &str,
    path: &str,
) -> Result<(String, Vec<DirEntry>), AppError> {
    let repo_record = crate::entity::repo::Entity::find_by_id(repo_id)
        .one(db)
        .await?
        .ok_or_else(|| AppError::NotFound("Repository not found".into()))?;

    let head_commit_id = match repo_record.head_commit_id {
        Some(id) => id,
        None => return Ok((String::new(), vec![])),
    };

    let head = crate::entity::commit::Entity::find()
        .filter(crate::entity::commit::Column::RepoId.eq(repo_id))
        .filter(crate::entity::commit::Column::CommitId.eq(&head_commit_id))
        .one(db)
        .await?
        .ok_or_else(|| AppError::NotFound("Head commit not found".into()))?;

    let dir_id = crate::storage::resolve_fs_id(db, repo_id, &head.root_id, path)
        .await
        .map_err(|e| AppError::internal(format!("resolve_fs_id failed: {e}")))?;

    let dir_data = read_fs_dir_data(db, repo_id, &dir_id).await?;

    Ok((
        dir_id,
        dir_data
            .dirents
            .into_iter()
            .map(|d| DirEntry {
                id: d.id,
                entry_type: if d.mode & S_IFDIR != 0 {
                    "dir".to_string()
                } else {
                    "file".to_string()
                },
                name: d.name,
                size: d.size,
                mtime: d.mtime,
                permission: "rw".to_string(),
            })
            .collect(),
    ))
}

// ─── Query / Request types ───────────────────────────────────────────────────

#[derive(Deserialize)]
pub struct FileOpsQuery {
    /// Parent directory path (for delete).
    pub p: Option<String>,
    /// If "true", include updated directory listing in response.
    pub reloaddir: Option<String>,
}

/// Response item for copy/move operations (matching seahub format).
#[derive(Serialize)]
pub struct CopyMoveResult {
    pub repo_id: String,
    pub parent_dir: String,
    pub obj_name: String,
}

/// Response for copy/move with reloaddir=true.
#[derive(Serialize)]
pub struct CopyMoveWithDirResult {
    pub repo_id: String,
    pub parent_dir: String,
    pub obj_name: String,
    pub dir_listing: Option<Vec<DirEntry>>,
}

// ─── Routes ──────────────────────────────────────────────────────────────────

pub fn fileops_routes() -> Router<Arc<AppState>> {
    Router::new()
        .route(
            "/{repo_id}/fileops/delete/",
            axum::routing::post(batch_delete_handler),
        )
        .route(
            "/{repo_id}/fileops/copy/",
            axum::routing::post(batch_copy_handler),
        )
        .route(
            "/{repo_id}/fileops/move/",
            axum::routing::post(batch_move_handler),
        )
}

// ─── Request body parsing ───────────────────────────────────────────────────

/// Parse the request body into a HashMap of form fields.
async fn parse_form_body(req: Request<Body>) -> Result<HashMap<String, String>, AppError> {
    let (_, body) = req.into_parts();
    let bytes = axum::body::to_bytes(body, usize::MAX)
        .await
        .map_err(|e| AppError::Internal(e.to_string()))?;
    serde_urlencoded::from_bytes(&bytes)
        .map_err(|_| AppError::BadRequest("invalid form data".into()))
}

// ─── Batch Delete ───────────────────────────────────────────────────────────

/// POST /api2/repos/{repo_id}/fileops/delete/?p={dir}&reloaddir=true
///
/// Form body: `file_names=file1.txt:file2.txt`
/// Response: 200 empty body (matching seahub).
/// With `reloaddir=true`: return updated directory listing.
pub async fn batch_delete_handler(
    auth: AuthUser,
    State(state): State<Arc<AppState>>,
    Path(repo_id): Path<String>,
    Query(query): Query<FileOpsQuery>,
    req: Request<Body>,
) -> Result<Response, AppError> {
    // Permission check
    crate::storage::check_repo_write_permission(state.db.as_ref(), &repo_id, auth.user_id).await?;

    let form = parse_form_body(req).await?;
    let file_names_str = form.get("file_names").map(|s| s.as_str()).unwrap_or("");
    let file_names = parse_file_names(file_names_str);

    if file_names.is_empty() {
        // Nothing to delete — return success.
        return Ok(Json(json!({})).into_response());
    }

    let parent_dir = normalize_path(query.p.as_deref().unwrap_or("/"));

    let db = state.db.as_ref();
    let head_root_id = get_head_root_id(db, &repo_id).await?;

    let parent_fs_id = crate::storage::resolve_fs_id(db, &repo_id, &head_root_id, &parent_dir)
        .await
        .map_err(|e| AppError::Internal(format!("resolve parent dir failed: {e}")))?;

    // Get total size of items being deleted (for repo size adjustment).
    let mut total_deleted: i64 = 0;
    for name in &file_names {
        let fp = if parent_dir == "/" {
            format!("/{name}")
        } else {
            format!("{parent_dir}/{name}")
        };
        if let Ok(sz) = crate::storage::get_entry_total_size(db, &repo_id, &fp).await {
            total_deleted += sz;
        }
    }

    // Read parent dirent metadata to determine obj_type per entry.
    let parent_data = crate::storage::read_fs_dir_data(db, &repo_id, &parent_fs_id)
        .await
        .map_err(|e| AppError::Internal(format!("read parent dir failed: {e}")))?;

    let names_to_delete = file_names.clone();
    FileOps::update_dir_tree_and_commit(
        db,
        &repo_id,
        &parent_dir,
        &parent_fs_id,
        &auth.email,
        &format!("Deleted {} items", names_to_delete.len()),
        crate::storage::file_ops::EMPTY_ANCESTOR_CHAIN,
        |dirents| {
            dirents.retain(|d| !names_to_delete.contains(&d.name));
            Ok(())
        },
    )
    .await
    .map_err(|e| AppError::Internal(e.to_string()))?;

    // Log activity for each deleted item (best-effort).
    for name in &file_names {
        let fp = if parent_dir == "/" {
            format!("/{name}")
        } else {
            format!("{parent_dir}/{name}")
        };
        let entry = parent_data.dirents.iter().find(|d| d.name == *name);
        let is_dir = entry.is_some_and(|d| d.mode & S_IFDIR != 0);
        activity_log::log_activity(
            db,
            &repo_id,
            "delete",
            if is_dir { "dir" } else { "file" },
            &fp,
            auth.user_id,
            None,
            entry.map(|d| d.size),
            entry.map(|d| d.id.as_str()),
        )
        .await;
    }

    // Remove from full-text search index.
    if let Some(indexer) = &state.indexer {
        for name in &file_names {
            let fp = if parent_dir == "/" {
                format!("/{name}")
            } else {
                format!("{parent_dir}/{name}")
            };
            if let Err(e) = indexer.delete_file(&repo_id, &fp) {
                tracing::warn!("Failed to delete index for {fp}: {e}");
            }
        }
    }

    // Adjust repo size (subtract total deleted size).
    crate::storage::adjust_repo_size(db, &repo_id, -total_deleted).await?;

    // Handle reloaddir=true
    if query.reloaddir.as_deref() == Some("true") {
        let (_, entries) = list_dir_from_fs_tree(db, &repo_id, &parent_dir).await?;
        return Ok(Json(json!({"dir_listing": entries})).into_response());
    }

    // Seahub returns empty body on delete success.
    Ok(StatusCode::OK.into_response())
}

// ─── Batch Copy ─────────────────────────────────────────────────────────────

/// POST /api2/repos/{repo_id}/fileops/copy/?p={dir}&reloaddir=true
///
/// Form body: `file_names=file1.txt:file2.txt&dst_repo={id}&dst_dir={path}`
/// Response: JSON list of `{repo_id, parent_dir, obj_name}` (matching seahub).
pub async fn batch_copy_handler(
    auth: AuthUser,
    State(state): State<Arc<AppState>>,
    Path(repo_id): Path<String>,
    Query(query): Query<FileOpsQuery>,
    req: Request<Body>,
) -> Result<Response, AppError> {
    let form = parse_form_body(req).await?;
    let file_names_str = form.get("file_names").map(|s| s.as_str()).unwrap_or("");
    let file_names = parse_file_names(file_names_str);

    if file_names.is_empty() {
        return Ok(Json(json!([])).into_response());
    }

    let dst_repo = form
        .get("dst_repo")
        .ok_or_else(|| AppError::BadRequest("dst_repo required".into()))?;
    let dst_dir = normalize_path(form.get("dst_dir").map(|s| s.as_str()).unwrap_or("/"));

    // Cross-repo copy is not supported yet.
    if *dst_repo != repo_id {
        return Err(AppError::BadRequest("cross-repo copy not supported".into()));
    }

    // Permission check
    crate::storage::check_repo_write_permission(state.db.as_ref(), &repo_id, auth.user_id).await?;

    let src_parent_dir = normalize_path(query.p.as_deref().unwrap_or("/"));
    let db = state.db.as_ref();
    let head_root_id = get_head_root_id(db, &repo_id).await?;

    // Resolve source parent directory
    let src_parent_fs_id =
        crate::storage::resolve_fs_id(db, &repo_id, &head_root_id, &src_parent_dir)
            .await
            .map_err(|e| AppError::Internal(format!("resolve source dir failed: {e}")))?;

    let src_parent_data = read_fs_dir_data(db, &repo_id, &src_parent_fs_id).await?;

    // Collect source entries with their metadata
    let mut new_entries: Vec<DirEntryData> = Vec::new();
    let now = chrono::Utc::now().timestamp();

    for name in &file_names {
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
    let dst_parent_fs_id = crate::storage::resolve_fs_id(db, &repo_id, &head_root_id, &dst_dir)
        .await
        .map_err(|e| AppError::Internal(format!("resolve dest dir failed: {e}")))?;

    // Read destination dir data to check for name collisions
    let dst_parent_data = read_fs_dir_data(db, &repo_id, &dst_parent_fs_id).await?;

    // Build result list with auto-rename on collision
    let mut results: Vec<CopyMoveResult> = Vec::new();
    let mut entries_to_add: Vec<DirEntryData> = Vec::new();

    for entry in &new_entries {
        let obj_name = if dst_parent_data.dirents.iter().any(|d| d.name == entry.name) {
            generate_unique_filename(&dst_parent_data.dirents, &entry.name)
        } else {
            entry.name.clone()
        };

        results.push(CopyMoveResult {
            repo_id: dst_repo.clone(),
            parent_dir: dst_dir.clone(),
            obj_name: obj_name.clone(),
        });

        entries_to_add.push(DirEntryData {
            name: obj_name,
            ..entry.clone()
        });
    }

    // Add entries to destination parent and create commit
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
        &repo_id,
        &dst_dir,
        &dst_parent_fs_id,
        &auth.email,
        &description,
        crate::storage::file_ops::EMPTY_ANCESTOR_CHAIN,
        |dirents| {
            for entry in &entries_to_add {
                // Check again in case dirents changed since we read
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

    // Log activity for each copied item (best-effort).
    for entry in &entries_to_add {
        let fp = if dst_dir == "/" {
            format!("/{}", entry.name)
        } else {
            format!("{}/{}", dst_dir, entry.name)
        };
        let obj_type = if entry.mode & S_IFDIR != 0 {
            "dir"
        } else {
            "file"
        };
        activity_log::log_activity(
            db,
            dst_repo,
            "create",
            obj_type,
            &fp,
            auth.user_id,
            None,
            Some(entry.size),
            Some(entry.id.as_str()),
        )
        .await;
    }

    // Index copied files in full-text search.
    if let Some(indexer) = &state.indexer {
        for entry in &entries_to_add {
            let fp = if dst_dir == "/" {
                format!("/{}", entry.name)
            } else {
                format!("{}/{}", dst_dir, entry.name)
            };
            if let Err(e) = indexer
                .reindex_file(db, &repo_id, &fp, &state.block_store)
                .await
            {
                tracing::warn!("Failed to index copied file {}: {e}", entry.name);
            }
        }
    }

    // Adjust repo size (add sizes of copied files).
    let total_copied: i64 = entries_to_add.iter().map(|e| e.size).sum();
    crate::storage::adjust_repo_size(db, &repo_id, total_copied).await?;

    // Handle reloaddir=true
    if query.reloaddir.as_deref() == Some("true") {
        let (_, entries) = list_dir_from_fs_tree(db, &repo_id, &dst_dir).await?;
        return Ok(Json(json!({
            "results": results,
            "dir_listing": entries,
        }))
        .into_response());
    }

    Ok(Json(json!(results)).into_response())
}

// ─── Batch Move ─────────────────────────────────────────────────────────────

/// POST /api2/repos/{repo_id}/fileops/move/?p={dir}&reloaddir=true
///
/// Form body: `file_names=file1.txt:file2.txt&dst_repo={id}&dst_dir={path}`
/// Response: JSON list of `{repo_id, parent_dir, obj_name}` (matching seahub).
pub async fn batch_move_handler(
    auth: AuthUser,
    State(state): State<Arc<AppState>>,
    Path(repo_id): Path<String>,
    Query(query): Query<FileOpsQuery>,
    req: Request<Body>,
) -> Result<Response, AppError> {
    let form = parse_form_body(req).await?;
    let file_names_str = form.get("file_names").map(|s| s.as_str()).unwrap_or("");
    let file_names = parse_file_names(file_names_str);

    if file_names.is_empty() {
        return Ok(Json(json!([])).into_response());
    }

    let dst_repo = form
        .get("dst_repo")
        .ok_or_else(|| AppError::BadRequest("dst_repo required".into()))?;
    let dst_dir = normalize_path(form.get("dst_dir").map(|s| s.as_str()).unwrap_or("/"));

    // Cross-repo move is not supported yet.
    if *dst_repo != repo_id {
        return Err(AppError::BadRequest("cross-repo move not supported".into()));
    }

    // Permission check
    crate::storage::check_repo_write_permission(state.db.as_ref(), &repo_id, auth.user_id).await?;

    let src_parent_dir = normalize_path(query.p.as_deref().unwrap_or("/"));
    let db = state.db.as_ref();
    let head_root_id = get_head_root_id(db, &repo_id).await?;

    // Resolve source parent directory
    let src_parent_fs_id =
        crate::storage::resolve_fs_id(db, &repo_id, &head_root_id, &src_parent_dir)
            .await
            .map_err(|e| AppError::Internal(format!("resolve source dir failed: {e}")))?;

    let src_parent_data = read_fs_dir_data(db, &repo_id, &src_parent_fs_id).await?;

    // Collect source entries with their metadata
    let mut entries_to_move: Vec<DirEntryData> = Vec::new();
    let now = chrono::Utc::now().timestamp();

    for name in &file_names {
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

    // Resolve destination parent directory
    let _dst_parent_fs_id = crate::storage::resolve_fs_id(db, &repo_id, &head_root_id, &dst_dir)
        .await
        .map_err(|e| AppError::Internal(format!("resolve dest dir failed: {e}")))?;

    // Step 1: Remove entries from source parent, create commit
    let src_names_for_closure: Vec<String> =
        entries_to_move.iter().map(|e| e.name.clone()).collect();

    let intermediate_root = FileOps::update_dir_tree_no_commit(
        db,
        &repo_id,
        &src_parent_dir,
        &src_parent_fs_id,
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

    FileOps::create_commit(db, &repo_id, &intermediate_root, &auth.email, &remove_desc)
        .await
        .map_err(|e| AppError::Internal(e.to_string()))?;

    // Step 2: Re-read head, resolve destination again, add entries with commit
    let new_head_root = get_head_root_id(db, &repo_id).await?;

    let new_dst_fs_id = crate::storage::resolve_fs_id(db, &repo_id, &new_head_root, &dst_dir)
        .await
        .map_err(|e| AppError::Internal(format!("resolve dest dir after removal failed: {e}")))?;

    // Read destination dir data to check for name collisions (in new tree)
    let new_dst_data = read_fs_dir_data(db, &repo_id, &new_dst_fs_id).await?;

    // Build result list with auto-rename on collision
    let mut results: Vec<CopyMoveResult> = Vec::new();
    let mut entries_to_add: Vec<DirEntryData> = Vec::new();

    for entry in &entries_to_move {
        let obj_name = if new_dst_data.dirents.iter().any(|d| d.name == entry.name) {
            generate_unique_filename(&new_dst_data.dirents, &entry.name)
        } else {
            entry.name.clone()
        };

        results.push(CopyMoveResult {
            repo_id: repo_id.clone(),
            parent_dir: dst_dir.clone(),
            obj_name: obj_name.clone(),
        });

        entries_to_add.push(DirEntryData {
            name: obj_name,
            ..entry.clone()
        });
    }

    FileOps::update_dir_tree_and_commit(
        db,
        &repo_id,
        &dst_dir,
        &new_dst_fs_id,
        &auth.email,
        &remove_desc,
        crate::storage::file_ops::EMPTY_ANCESTOR_CHAIN,
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

    // Log activity for each moved item (best-effort).
    for entry in &entries_to_add {
        let old_fp = if src_parent_dir == "/" {
            format!("/{}", entry.name)
        } else {
            format!("{src_parent_dir}/{}", entry.name)
        };
        let new_fp = if dst_dir == "/" {
            format!("/{}", entry.name)
        } else {
            format!("{}/{}", dst_dir, entry.name)
        };
        let obj_type = if entry.mode & S_IFDIR != 0 {
            "dir"
        } else {
            "file"
        };
        activity_log::log_activity(
            db,
            &repo_id,
            "move",
            obj_type,
            &new_fp,
            auth.user_id,
            Some(&old_fp),
            Some(entry.size),
            Some(entry.id.as_str()),
        )
        .await;
    }

    // Update full-text search index for each moved item.
    if let Some(indexer) = &state.indexer {
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
            if let Err(e) = indexer.delete_file(&repo_id, &old_fp) {
                tracing::warn!("Failed to delete old index on batch move: {e}");
            }
            if let Err(e) = indexer
                .reindex_file(db, &repo_id, &new_fp, &state.block_store)
                .await
            {
                tracing::warn!("Failed to reindex on batch move: {e}");
            }
        }
    }

    // Handle reloaddir=true
    if query.reloaddir.as_deref() == Some("true") {
        let (_, entries) = list_dir_from_fs_tree(db, &repo_id, &dst_dir).await?;
        return Ok(Json(json!({
            "results": results,
            "dir_listing": entries,
        }))
        .into_response());
    }

    Ok(Json(json!(results)).into_response())
}
