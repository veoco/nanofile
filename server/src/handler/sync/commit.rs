use axum::{
    Json, Router,
    extract::{Path, State},
    http::StatusCode,
};
use rand::RngExt;
use serde::Serialize;
use std::collections::HashMap;
use std::sync::Arc;

use crate::repository::FsObjectRepository;

use crate::AppState;
use crate::middleware::auth::SyncAuth;
use base::common::{DirEntryData, FsDirData, FsFileData};
use base::error::AppError;
use infra::activity_log;
use infra::common::EMPTY_SHA1;
use infra::serialization::S_IFDIR;

/// Result of block checking and size delta computation.
struct CheckResult {
    missing: Vec<String>,
    size_delta: i64,
}

const EXCLUDED_ACTIVITY_PREFIXES: &[&str] = &["/_Internal", "/images/sdoc", "/images/auto-upload"];
const MAX_BRANCH_RETRY: u32 = 3;

#[derive(Serialize)]
pub struct HeadCommitResponse {
    pub is_corrupted: i32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub head_commit_id: Option<String>,
}

pub fn commit_routes() -> Router<Arc<AppState>> {
    Router::new()
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
    let svc = state.sync_service();
    let repo_model = svc
        .find_repo(&repo_id)
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
    let svc = state.sync_service();
    let repo_model = svc
        .find_repo(&repo_id)
        .await?
        .ok_or_else(|| AppError::NotFound("repo not found".into()))?;

    if commit_id == "0000000000000000000000000000000000000000" {
        let empty_commit = base::common::CommitData {
            commit_id: commit_id.clone(),
            repo_id: repo_id.clone(),
            root_id: "0000000000000000000000000000000000000000".to_string(),
            creator_name: "".to_string(),
            creator: "0000000000000000000000000000000000000000".to_string(),
            description: "".to_string(),
            ctime: 0,
            parent_id: None,
            second_parent_id: None,
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
            version: 1,
        };
        let json = crate::domain::commit::to_json(&empty_commit);
        return Ok(json.into_bytes());
    }

    let commit_model = svc
        .find_commit(&repo_id, &commit_id)
        .await?
        .ok_or_else(|| AppError::NotFound("commit not found".into()))?;

    let commit_data = base::common::CommitData {
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

    let json = crate::domain::commit::to_json(&commit_data);
    Ok(json.into_bytes())
}

pub async fn put_commit(
    State(state): State<Arc<AppState>>,
    _auth: SyncAuth,
    Path((repo_id, _commit_id)): Path<(String, String)>,
    body: axum::body::Body,
) -> Result<StatusCode, AppError> {
    let max_bytes = (state.config.server.max_upload_size_mb * 1024 * 1024) as usize;
    let data = axum::body::to_bytes(body, max_bytes)
        .await
        .map_err(|e| AppError::Internal(e.to_string()))?;

    let commit_data: base::common::CommitData = serde_json::from_slice(&data)
        .map_err(|e| AppError::Internal(format!("invalid commit JSON: {}", e)))?;

    if commit_data.repo_id != repo_id {
        return Err(AppError::BadRequest("repo_id mismatch".into()));
    }

    state.sync_service().put_commit(&commit_data).await?;
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

    if new_head.len() != 40 || !new_head.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err(AppError::BadRequest("invalid commit id".into()));
    }

    crate::domain::permission::check_repo_write_permission(
        state.repos.member.as_ref(),
        &repo_id,
        _auth.user_id,
    )
    .await?;

    let svc = state.sync_service();
    let new_commit = svc
        .find_commit(&repo_id, new_head)
        .await?
        .ok_or_else(|| AppError::Internal("commit not found".into()))?;

    let base_root_id: Option<String> = if let Some(ref parent_id) = new_commit.parent_id
        && parent_id != "0000000000000000000000000000000000000000"
    {
        svc.find_commit(&repo_id, parent_id)
            .await?
            .map(|c| c.root_id)
    } else {
        None
    };

    let check_result = check_commit_blocks(
        state.repos.fs_object.clone(),
        &state.repos,
        state.block_store.clone(),
        &repo_id,
        &new_commit.root_id,
        base_root_id.as_deref(),
    )
    .await?;
    if !check_result.missing.is_empty() {
        return Err(AppError::BlockMissing);
    }

    infra::storage::check_commit_file_locks(
        state.db.as_ref(),
        &repo_id,
        &new_commit.root_id,
        _auth.user_id,
    )
    .await?;

    if let Some(ref parent_id) = new_commit.parent_id
        && !svc.parent_commit_exists(&repo_id, parent_id).await?
    {
        return Err(AppError::BadRequest("parent commit not found".into()));
    }

    let mut attempt: u32 = 0;

    loop {
        attempt += 1;

        let current_head = svc
            .find_repo(&repo_id)
            .await?
            .ok_or_else(|| AppError::Internal("repo not found".into()))?
            .head_commit_id;

        let is_same_commit = current_head.as_deref() == Some(new_head.as_str());
        if is_same_commit {
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
                let delay_ms = rand::rng().random_range(100..=500);
                tokio::time::sleep(std::time::Duration::from_millis(delay_ms)).await;
                continue;
            }
            break Err(AppError::Conflict(
                "commit parent_id does not match current HEAD".into(),
            ));
        }

        svc.update_head_commit(&repo_id, Some(new_head.clone()))
            .await?;

        crate::fs::core::adjust_repo_size(
            state.db.as_ref(),
            &state.repos,
            &repo_id,
            check_result.size_delta,
        )
        .await?;

        let is_wiki = svc.is_wiki_repo(&repo_id).await?;

        let old_root = if let Some(ref parent_id) = new_commit.parent_id
            && parent_id != "0000000000000000000000000000000000000000"
        {
            svc.find_commit(&repo_id, parent_id)
                .await?
                .map(|c| c.root_id)
        } else {
            None
        };

        let mut changes = crate::fs::core::tree_diff::diff_trees(
            &state.repos,
            &repo_id,
            old_root.as_deref(),
            &new_commit.root_id,
        )
        .await
        .unwrap_or_default();

        if !is_wiki {
            let commit_desc = new_commit.description.as_str();
            let is_reverted =
                commit_desc.starts_with("Reverted") || commit_desc.starts_with("Recovered");

            for change in &mut changes {
                if is_reverted && (change.op_type == "create" || change.op_type == "edit") {
                    change.op_type = "recover";
                }

                if EXCLUDED_ACTIVITY_PREFIXES
                    .iter()
                    .any(|p| change.path.starts_with(p))
                {
                    continue;
                }

                activity_log::log_activity(
                    state.db.as_ref(),
                    &repo_id,
                    change.op_type,
                    change.obj_type,
                    &change.path,
                    _auth.user_id,
                    change.old_path.as_deref(),
                    Some(change.size),
                    Some(&change.obj_id),
                    None,
                    None,
                )
                .await;
            }
        }

        if let Some(ref indexer) = state.indexer {
            for change in &changes {
                if change.obj_type != "file" {
                    continue;
                }
                match change.op_type {
                    "create" | "edit" | "recover" => {
                        if let Err(e) = indexer
                            .reindex_file(
                                state.db.as_ref(),
                                &repo_id,
                                &change.path,
                                &state.block_store,
                            )
                            .await
                        {
                            tracing::warn!("sync index file {}: {e}", change.path);
                        }
                    }
                    "delete" => {
                        if let Err(e) = indexer.delete_file(&repo_id, &change.path) {
                            tracing::warn!("sync delete index {}: {e}", change.path);
                        }
                    }
                    "rename" | "move" => {
                        if let Some(ref old_path) = change.old_path {
                            let _ = indexer.delete_file(&repo_id, old_path);
                        }
                        if let Err(e) = indexer
                            .reindex_file(
                                state.db.as_ref(),
                                &repo_id,
                                &change.path,
                                &state.block_store,
                            )
                            .await
                        {
                            tracing::warn!("sync reindex {}: {e}", change.path);
                        }
                    }
                    _ => {}
                }
            }
        }

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

// ── Block checking helpers (sync protocol internals) ────────────────────

struct DiffFrame {
    base_fs_id: Option<String>,
    new_fs_id: String,
    prefix: String,
}

/// Verify blocks referenced by the new commit exist on the server.
async fn check_commit_blocks(
    fs_object_repo: Arc<dyn FsObjectRepository>,
    repos: &crate::repository::Repositories,
    block_store: infra::storage::DynBlockStorage,
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
            return Ok(CheckResult {
                missing: Vec::new(),
                size_delta: 0,
            });
        }

        let mut stack: Vec<DiffFrame> = vec![DiffFrame {
            base_fs_id: Some(base_root.to_string()),
            new_fs_id: new_root_id.to_string(),
            prefix: String::new(),
        }];

        while let Some(frame) = stack.pop() {
            let Some(ref base_fs) = frame.base_fs_id else {
                let new_dir: FsDirData =
                    match crate::fs::core::read_fs_dir_data(repos, repo_id, &frame.new_fs_id).await
                    {
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
                            fs_object_repo.clone(),
                            block_store.clone(),
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
                continue;
            }
            if *base_fs == EMPTY_SHA1 {
                stack.push(DiffFrame {
                    base_fs_id: None,
                    new_fs_id: frame.new_fs_id,
                    prefix: frame.prefix,
                });
                continue;
            }

            let base_dir: FsDirData =
                match crate::fs::core::read_fs_dir_data(repos, repo_id, base_fs).await {
                    Ok(d) => d,
                    Err(_) => continue,
                };
            let new_dir: FsDirData =
                match crate::fs::core::read_fs_dir_data(repos, repo_id, &frame.new_fs_id).await {
                    Ok(d) => d,
                    Err(_) => continue,
                };

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
                        if is_dir {
                            stack.push(DiffFrame {
                                base_fs_id: None,
                                new_fs_id: new_entry.id.clone(),
                                prefix: child,
                            });
                        } else {
                            size_delta += new_entry.size;
                            check_file_blocks(
                                fs_object_repo.clone(),
                                block_store.clone(),
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
                            continue;
                        }
                        if is_dir && (base_entry.mode & S_IFDIR != 0) {
                            stack.push(DiffFrame {
                                base_fs_id: Some(base_entry.id.clone()),
                                new_fs_id: new_entry.id.clone(),
                                prefix: child,
                            });
                        } else {
                            size_delta += new_entry.size - base_entry.size;
                            check_file_blocks(
                                fs_object_repo.clone(),
                                block_store.clone(),
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
        }
    } else {
        full_check_blocks(
            fs_object_repo.clone(),
            block_store.clone(),
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

async fn full_check_blocks(
    fs_object_repo: Arc<dyn FsObjectRepository>,
    block_store: infra::storage::DynBlockStorage,
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
        let obj = match fs_object_repo
            .find_by_repo_and_fs_id(repo_id, &fs_id)
            .await?
        {
            Some(o) => o,
            None => continue,
        };
        if obj.obj_type == 1 {
            let file_data: FsFileData = serde_json::from_str(&obj.data)
                .map_err(|e| AppError::Internal(format!("invalid file object: {e}")))?;
            *size_total += file_data.size;
            if check_blocks_concurrent(block_store.clone(), &file_data.block_ids).await {
                missing.push(path.clone());
            }
        } else if obj.obj_type == 3 {
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

async fn check_file_blocks(
    fs_object_repo: Arc<dyn FsObjectRepository>,
    block_store: infra::storage::DynBlockStorage,
    repo_id: &str,
    fs_id: &str,
    path: &str,
    missing: &mut Vec<String>,
) -> Result<(), AppError> {
    let obj = match fs_object_repo
        .find_by_repo_and_fs_id(repo_id, fs_id)
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
    if check_blocks_concurrent(block_store, &file_data.block_ids).await {
        missing.push(path.to_string());
    }
    Ok(())
}

async fn check_blocks_concurrent(
    block_store: infra::storage::DynBlockStorage,
    block_ids: &[String],
) -> bool {
    const BATCH_SIZE: usize = 8;
    for chunk in block_ids.chunks(BATCH_SIZE) {
        let futures: Vec<_> = chunk
            .iter()
            .map(|block_id| {
                let store = block_store.clone();
                let id = block_id.clone();
                async move { !store.has_block(&id).await }
            })
            .collect();
        let results = futures::future::join_all(futures).await;
        if results.into_iter().any(|missing| missing) {
            return true;
        }
    }
    false
}
