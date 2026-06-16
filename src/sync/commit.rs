use axum::{
    Json, Router,
    extract::{Path, State},
    http::StatusCode,
};
use rand::RngExt;
use sea_orm::{ActiveModelTrait, ColumnTrait, EntityTrait, QueryFilter};
use serde::Serialize;
use std::collections::HashMap;
use std::sync::Arc;

use crate::AppState;
use crate::auth::middleware::SyncAuth;
use crate::entity::{commit, fs_object, repo};
use crate::error::AppError;
use crate::serialization::S_IFDIR;
use crate::serialization::fs_json::{DirEntryData, FsDirData, FsFileData};
use crate::storage::EMPTY_SHA1;

/// Result of block checking and size delta computation.
struct CheckResult {
    /// Relative paths of files with missing blocks.
    missing: Vec<String>,
    /// Size delta between the base tree and the new tree.
    /// Positive = files grew, negative = files shrank/deleted.
    size_delta: i64,
}

/// Maximum number of retries when branch update conflicts.
const MAX_BRANCH_RETRY: u32 = 3;

#[derive(Serialize)]
pub struct HeadCommitResponse {
    pub is_corrupted: i32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub head_commit_id: Option<String>,
}

pub fn commit_routes() -> Router<Arc<AppState>> {
    Router::new()
        // Both with and without trailing slash — seaf-daemon uses HEAD/ for
        // update_branch (PUT .../commit/HEAD/?head=...) but HEAD for
        // get_head_commit (GET .../commit/HEAD).
        .route(
            "/{repo_id}/commit/HEAD",
            axum::routing::get(get_head_commit).put(update_branch),
        )
        .route(
            "/{repo_id}/commit/HEAD/",
            axum::routing::get(get_head_commit).put(update_branch),
        )
        .route(
            "/{repo_id}/commit/{commit_id}",
            axum::routing::get(get_commit).put(put_commit),
        )
}

pub async fn get_head_commit(
    State(state): State<Arc<AppState>>,
    _auth: SyncAuth,
    Path(repo_id): Path<String>,
) -> Result<Json<HeadCommitResponse>, AppError> {
    let repo_model = repo::Entity::find_by_id(&repo_id)
        .one(state.db.as_ref())
        .await?
        .ok_or_else(|| AppError::NotFound("repo not found".into()))?;

    let head_commit_id = repo_model
        .head_commit_id
        .unwrap_or_else(|| "0000000000000000000000000000000000000000".to_string());

    Ok(Json(HeadCommitResponse {
        is_corrupted: 0,
        head_commit_id: Some(head_commit_id),
    }))
}

pub async fn get_commit(
    State(state): State<Arc<AppState>>,
    _auth: SyncAuth,
    Path((repo_id, commit_id)): Path<(String, String)>,
) -> Result<Vec<u8>, AppError> {
    // The zero commit ID represents an empty repository — return a minimal
    // placeholder commit so the client can proceed with the initial sync.
    if commit_id == "0000000000000000000000000000000000000000" {
        let empty_commit = crate::serialization::commit_json::CommitData {
            commit_id: commit_id.clone(),
            repo_id: repo_id.clone(),
            root_id: "0000000000000000000000000000000000000000".to_string(),
            creator_name: "".to_string(),
            creator: "0000000000000000000000000000000000000000".to_string(),
            description: "".to_string(),
            ctime: 0,
            parent_id: None,
            second_parent_id: None,
            repo_name: None,
            repo_desc: None,
            repo_category: None,
            encrypted: None,
            enc_version: None,
            magic: None,
            key: None,
            version: 1,
        };
        let json = empty_commit.to_compact_json();
        return Ok(json.into_bytes());
    }

    let commit_model = commit::Entity::find()
        .filter(commit::Column::RepoId.eq(&repo_id))
        .filter(commit::Column::CommitId.eq(&commit_id))
        .one(state.db.as_ref())
        .await?
        .ok_or_else(|| AppError::NotFound("commit not found".into()))?;

    // Fetch repo metadata — the seaf-daemon uses repo_name/repo_desc from
    // the commit JSON (via seaf_repo_from_commit) to populate the local
    // library name. Without these fields the library shows as "(unnamed)".
    let repo_model = repo::Entity::find_by_id(&repo_id)
        .one(state.db.as_ref())
        .await?
        .ok_or_else(|| AppError::NotFound("repo not found".into()))?;

    let commit_data = crate::serialization::commit_json::CommitData {
        commit_id: commit_model.commit_id.clone(),
        repo_id: commit_model.repo_id.clone(),
        root_id: commit_model.root_id.clone(),
        creator_name: commit_model.creator_name.clone(),
        creator: "0000000000000000000000000000000000000000".to_string(),
        description: commit_model.description.clone(),
        ctime: commit_model.ctime,
        parent_id: commit_model.parent_id.clone(),
        second_parent_id: commit_model.second_parent_id.clone(),
        repo_name: Some(repo_model.name.clone()),
        repo_desc: Some(repo_model.description.clone()),
        repo_category: None,
        encrypted: if repo_model.encrypted == 1 {
            Some("true".to_string())
        } else {
            None
        },
        enc_version: Some(repo_model.enc_version as i32),
        magic: repo_model.magic.clone(),
        key: repo_model.random_key.clone(),
        version: commit_model.version as i32,
    };

    let json = commit_data.to_compact_json();
    Ok(json.into_bytes())
}

pub async fn put_commit(
    State(state): State<Arc<AppState>>,
    _auth: SyncAuth,
    Path((repo_id, commit_id)): Path<(String, String)>,
    body: axum::body::Body,
) -> Result<StatusCode, AppError> {
    let max_bytes = (state.config.server.max_upload_size_mb * 1024 * 1024) as usize;
    let data = axum::body::to_bytes(body, max_bytes)
        .await
        .map_err(|e| AppError::Internal(e.to_string()))?;

    let commit_data: crate::serialization::commit_json::CommitData = serde_json::from_slice(&data)
        .map_err(|e| AppError::Internal(format!("invalid commit JSON: {}", e)))?;

    if commit_data.repo_id != repo_id {
        return Err(AppError::BadRequest("repo_id mismatch".into()));
    }

    let existing = commit::Entity::find()
        .filter(commit::Column::RepoId.eq(&repo_id))
        .filter(commit::Column::CommitId.eq(&commit_id))
        .one(state.db.as_ref())
        .await?;

    if existing.is_none() {
        let commit_model = commit::ActiveModel {
            id: sea_orm::NotSet,
            repo_id: sea_orm::Set(commit_data.repo_id),
            commit_id: sea_orm::Set(commit_data.commit_id),
            root_id: sea_orm::Set(commit_data.root_id),
            parent_id: sea_orm::Set(commit_data.parent_id),
            second_parent_id: sea_orm::Set(commit_data.second_parent_id),
            creator_name: sea_orm::Set(commit_data.creator_name),
            description: sea_orm::Set(commit_data.description),
            ctime: sea_orm::Set(commit_data.ctime),
            version: sea_orm::Set(commit_data.version as i8),
        };
        commit::Entity::insert(commit_model)
            .exec(state.db.as_ref())
            .await?;
    }

    Ok(StatusCode::OK)
}

pub async fn update_branch(
    State(state): State<Arc<AppState>>,
    _auth: SyncAuth,
    Path(repo_id): Path<String>,
    axum::extract::Query(params): axum::extract::Query<std::collections::HashMap<String, String>>,
) -> Result<StatusCode, AppError> {
    let new_head = params
        .get("head")
        .ok_or_else(|| AppError::BadRequest("missing head parameter".into()))?;

    // Validate commit_id format (40 hex characters, case-insensitive).
    // This matches seafile's is_object_id_valid() which uses
    // strspn(id, "0123456789abcdefABCDEF") == 40.
    if new_head.len() != 40 || !new_head.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err(AppError::BadRequest("invalid commit id".into()));
    }

    // Permission check: verify write access to the repo.
    // SyncAuth confirms the token is valid; this checks repo-level
    // write permission (seafile-server put_update_branch_cb line 1323-1328).
    crate::storage::check_repo_write_permission(state.db.as_ref(), &repo_id, _auth.user_id).await?;

    // Read the new commit (checks existence + gets parent_id for conflict detection).
    let new_commit = commit::Entity::find()
        .filter(commit::Column::RepoId.eq(&repo_id))
        .filter(commit::Column::CommitId.eq(new_head))
        .one(state.db.as_ref())
        .await?
        .ok_or_else(|| AppError::Internal("commit not found".into()))?;

    // Verify all blocks referenced by the new commit exist on the server.
    // Uses tree-diff against the parent commit so only changed files are checked.
    // This matches seafile-server's check_blocks() which calls diff_trees()
    // to compare base vs remote and only inspects files that differ.
    let base_root_id: Option<String> = if let Some(ref parent_id) = new_commit.parent_id
        && parent_id != "0000000000000000000000000000000000000000"
    {
        commit::Entity::find()
            .filter(commit::Column::RepoId.eq(&repo_id))
            .filter(commit::Column::CommitId.eq(parent_id))
            .one(state.db.as_ref())
            .await?
            .map(|c| c.root_id)
    } else {
        None
    };

    let check_result = check_commit_blocks(
        state.db.as_ref(),
        state.block_store.as_ref(),
        &repo_id,
        &new_commit.root_id,
        base_root_id.as_deref(),
    )
    .await?;
    if !check_result.missing.is_empty() {
        return Err(AppError::BlockMissing);
    }

    // File lock check: verify no file in the commit is locked by another user.
    // The daemon parses 403 + "File <path> is locked" as SYNC_ERROR_ID_FILE_LOCKED.
    crate::storage::check_commit_file_locks(
        state.db.as_ref(),
        &repo_id,
        &new_commit.root_id,
        _auth.user_id,
    )
    .await?;

    // Verify the parent commit exists. Seafile requires the base commit
    // to be present on the server; a missing parent is a client error.
    if let Some(ref parent_id) = new_commit.parent_id
        && parent_id != "0000000000000000000000000000000000000000"
    {
        let parent_exists = commit::Entity::find()
            .filter(commit::Column::RepoId.eq(&repo_id))
            .filter(commit::Column::CommitId.eq(parent_id))
            .one(state.db.as_ref())
            .await?
            .is_some();
        if !parent_exists {
            return Err(AppError::BadRequest("parent commit not found".into()));
        }
    }

    // CAS retry loop: re-read current HEAD and attempt update.
    // Seafile's test_and_update_branch uses a DB transaction with
    // SELECT ... FOR UPDATE. Since nanofile uses SQLite (serialized
    // writes), a loop with re-read + conditional update is sufficient.
    let mut attempt: u32 = 0;

    loop {
        attempt += 1;

        let current_head = repo::Entity::find_by_id(&repo_id)
            .one(state.db.as_ref())
            .await?
            .ok_or_else(|| AppError::Internal("repo not found".into()))?
            .head_commit_id;

        // Conflict detection: the new commit's parent MUST match the current
        // HEAD (unless this is setting the same commit — which is idempotent).
        //
        // When there is no existing HEAD (current_head is None, meaning the
        // repo was created via REST API without an initial commit), accept
        // any first commit.
        let is_same_commit = current_head.as_deref() == Some(new_head.as_str());
        if is_same_commit {
            // Idempotent: HEAD already points to this commit → success.
            if let Some(ref notif_mgr) = state.notification_manager {
                notif_mgr
                    .notify(crate::notification::events::RepoUpdateEvent::new(
                        repo_id.clone(),
                        new_head.clone(),
                    ))
                    .await;
            }
            break Ok(StatusCode::OK);
        }

        if current_head.is_some() && new_commit.parent_id != current_head {
            if attempt < MAX_BRANCH_RETRY {
                // Stale HEAD — retry with random backoff (100-500ms).
                let delay_ms = rand::rng().random_range(100..=500);
                tokio::time::sleep(std::time::Duration::from_millis(delay_ms)).await;
                continue;
            }
            break Err(AppError::Conflict(
                "commit parent_id does not match current HEAD".into(),
            ));
        }

        let mut repo_active: repo::ActiveModel = repo::Entity::find_by_id(&repo_id)
            .one(state.db.as_ref())
            .await?
            .ok_or_else(|| AppError::Internal("repo not found".into()))?
            .into();

        repo_active.head_commit_id = sea_orm::Set(Some(new_head.clone()));
        repo_active.update(state.db.as_ref()).await?;

        // Compute repo size delta from the tree-diff result, avoiding a
        // full BFS traversal of every file.  When `size_delta` is 0 and
        // there's no prior size (first commit), `adjust_repo_size` falls
        // back to `compute_repo_size()` automatically.
        crate::storage::adjust_repo_size(state.db.as_ref(), &repo_id, check_result.size_delta)
            .await?;

        // Success — notify listeners.
        if let Some(ref notif_mgr) = state.notification_manager {
            notif_mgr
                .notify(crate::notification::events::RepoUpdateEvent::new(
                    repo_id.clone(),
                    new_head.clone(),
                ))
                .await;
        }
        break Ok(StatusCode::OK);
    }
}

/// Struct for stack-based tree-diff traversal.
struct DiffFrame {
    /// base fs_id (None → entirely new subtree)
    base_fs_id: Option<String>,
    /// new commit's fs_id for this subtree
    new_fs_id: String,
    /// relative path prefix (e.g. "docs/images/")
    prefix: String,
}

/// Verify blocks referenced by the new commit exist on the server.
///
/// When `base_root_id` is provided (non-first commit), uses **tree-diff**
/// between base and new roots — only files that differ between the two
/// trees are inspected. Unchanged subtrees are skipped entirely.
///
/// When `base_root_id` is `None` (first commit or empty parent), falls back
/// to a full traversal of the new tree.
///
/// This matches seafile-server's `check_blocks()` which calls
/// `diff_trees(base_root, remote_root)` and only invokes `check_file_blocks`
/// for files whose IDs differ.
async fn check_commit_blocks(
    db: &sea_orm::DatabaseConnection,
    block_store: &dyn crate::storage::BlockStorageBackend,
    repo_id: &str,
    new_root_id: &str,
    base_root_id: Option<&str>,
) -> Result<CheckResult, AppError> {
    if new_root_id == "0000000000000000000000000000000000000000" {
        return Ok(CheckResult {
            missing: Vec::new(),
            size_delta: 0,
        });
    }

    let mut missing: Vec<String> = Vec::new();
    let mut size_delta: i64 = 0;

    if let Some(base_root) = base_root_id {
        if base_root == new_root_id {
            // Same root — no changes at all, skip entirely.
            return Ok(CheckResult {
                missing: Vec::new(),
                size_delta: 0,
            });
        }

        // Stack-based tree-diff traversal.
        let mut stack: Vec<DiffFrame> = vec![DiffFrame {
            base_fs_id: Some(base_root.to_string()),
            new_fs_id: new_root_id.to_string(),
            prefix: String::new(),
        }];

        while let Some(frame) = stack.pop() {
            let Some(ref base_fs) = frame.base_fs_id else {
                // Entirely new subtree — walk all files.
                let new_dir: FsDirData =
                    match crate::storage::read_fs_dir_data(db, repo_id, &frame.new_fs_id).await {
                        Ok(d) => d,
                        Err(_) => continue,
                    };
                for entry in &new_dir.dirents {
                    let child = if frame.prefix.is_empty() {
                        entry.name.clone()
                    } else {
                        format!("{}/{}", frame.prefix, entry.name)
                    };
                    if entry.mode & S_IFDIR != 0 {
                        stack.push(DiffFrame {
                            base_fs_id: None,
                            new_fs_id: entry.id.clone(),
                            prefix: child,
                        });
                    } else {
                        size_delta += entry.size;
                        check_file_blocks(
                            block_store,
                            db,
                            repo_id,
                            &entry.id,
                            &child,
                            &mut missing,
                        )
                        .await?;
                    }
                }
                continue;
            };

            if *base_fs == frame.new_fs_id {
                // Same fs_id — entire subtree unchanged, skip.
                continue;
            }

            if *base_fs == EMPTY_SHA1 {
                // Base was empty — handle as entirely new subtree.
                stack.push(DiffFrame {
                    base_fs_id: None,
                    new_fs_id: frame.new_fs_id,
                    prefix: frame.prefix,
                });
                continue;
            }

            // Load both directories.
            let base_dir: FsDirData =
                match crate::storage::read_fs_dir_data(db, repo_id, base_fs).await {
                    Ok(d) => d,
                    Err(_) => continue,
                };
            let new_dir: FsDirData =
                match crate::storage::read_fs_dir_data(db, repo_id, &frame.new_fs_id).await {
                    Ok(d) => d,
                    Err(_) => continue,
                };

            // Index base entries by name for O(1) lookups.
            let base_map: HashMap<&str, &DirEntryData> = base_dir
                .dirents
                .iter()
                .map(|d| (d.name.as_str(), d))
                .collect();

            for new_entry in &new_dir.dirents {
                let child = if frame.prefix.is_empty() {
                    new_entry.name.clone()
                } else {
                    format!("{}/{}", frame.prefix, new_entry.name)
                };

                let is_dir = new_entry.mode & S_IFDIR != 0;

                match base_map.get(new_entry.name.as_str()) {
                    None => {
                        // Entry only in new tree — added.
                        if is_dir {
                            stack.push(DiffFrame {
                                base_fs_id: None,
                                new_fs_id: new_entry.id.clone(),
                                prefix: child,
                            });
                        } else {
                            size_delta += new_entry.size;
                            check_file_blocks(
                                block_store,
                                db,
                                repo_id,
                                &new_entry.id,
                                &child,
                                &mut missing,
                            )
                            .await?;
                        }
                    }
                    Some(base_entry) => {
                        if new_entry.id == base_entry.id {
                            // Same id — unchanged.
                            continue;
                        }
                        let base_is_dir = base_entry.mode & S_IFDIR != 0;
                        if is_dir && base_is_dir {
                            // Both directories — recurse to compare children.
                            stack.push(DiffFrame {
                                base_fs_id: Some(base_entry.id.clone()),
                                new_fs_id: new_entry.id.clone(),
                                prefix: child,
                            });
                        } else {
                            // File modified, or file↔dir type change.
                            size_delta += new_entry.size - base_entry.size;
                            check_file_blocks(
                                block_store,
                                db,
                                repo_id,
                                &new_entry.id,
                                &child,
                                &mut missing,
                            )
                            .await?;
                        }
                    }
                }
            }
            // Entries only in base tree (deleted) — no block check needed.
            // Their size contribution is already removed from size_delta
            // (since we added new size but didn't subtract deleted ones).
            // This means size_delta for deleted files is ignored — the
            // existing `adjust_repo_size` handles this via `repo.size` delta.
        }
    } else {
        // No base commit (first commit) — full traversal fallback.
        full_check_blocks(
            db,
            block_store,
            repo_id,
            new_root_id,
            &mut missing,
            &mut size_delta,
        )
        .await?;
    }

    Ok(CheckResult {
        missing,
        size_delta,
    })
}

/// Full tree traversal of all files. Used when there is no base commit
/// to diff against (first commit). Also accumulates the total size for
/// repos that haven't been sized yet.
async fn full_check_blocks(
    db: &sea_orm::DatabaseConnection,
    block_store: &dyn crate::storage::BlockStorageBackend,
    repo_id: &str,
    root_id: &str,
    missing: &mut Vec<String>,
    size_total: &mut i64,
) -> Result<(), AppError> {
    let mut stack: Vec<(String, String)> = vec![(root_id.to_string(), String::new())];

    while let Some((fs_id, path)) = stack.pop() {
        if fs_id == "0000000000000000000000000000000000000000" {
            continue;
        }

        let obj = match fs_object::Entity::find()
            .filter(fs_object::Column::RepoId.eq(repo_id))
            .filter(fs_object::Column::FsId.eq(&fs_id))
            .one(db)
            .await?
        {
            Some(o) => o,
            None => continue,
        };

        if obj.obj_type == 1 {
            // File — check blocks and accumulate size.
            let file_data: FsFileData = serde_json::from_str(&obj.data)
                .map_err(|e| AppError::Internal(format!("invalid file object: {e}")))?;
            *size_total += file_data.size;
            for block_id in &file_data.block_ids {
                if !block_store.has_block(block_id).await {
                    missing.push(path.clone());
                    break;
                }
            }
        } else if obj.obj_type == 3 {
            // Directory — push children.
            let dir_data: FsDirData = serde_json::from_str(&obj.data)
                .map_err(|e| AppError::Internal(format!("invalid dir object: {e}")))?;
            for entry in &dir_data.dirents {
                let child_path = if path.is_empty() {
                    entry.name.clone()
                } else {
                    format!("{}/{}", path, entry.name)
                };
                stack.push((entry.id.clone(), child_path));
            }
        }
    }

    Ok(())
}

/// Check blocks for a single file identified by its fs_id.
/// If any block is missing, the file's relative path is appended to `missing`.
async fn check_file_blocks(
    block_store: &dyn crate::storage::BlockStorageBackend,
    db: &sea_orm::DatabaseConnection,
    repo_id: &str,
    fs_id: &str,
    path: &str,
    missing: &mut Vec<String>,
) -> Result<(), AppError> {
    let obj = match fs_object::Entity::find()
        .filter(fs_object::Column::RepoId.eq(repo_id))
        .filter(fs_object::Column::FsId.eq(fs_id))
        .one(db)
        .await?
    {
        Some(o) => o,
        None => return Ok(()),
    };

    if obj.obj_type != 1 {
        return Ok(());
    }

    let file_data: FsFileData = serde_json::from_str(&obj.data)
        .map_err(|e| AppError::Internal(format!("invalid file object: {e}")))?;

    for block_id in &file_data.block_ids {
        if !block_store.has_block(block_id).await {
            missing.push(path.to_string());
            break;
        }
    }

    Ok(())
}
