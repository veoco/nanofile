//! One delete path for files, directories and batch requests.
//!
//! The four delete entrypoints (v1 file, v1 directory, v2.1/WebDAV dirent, batch)
//! had each grown their own copy of the same sequence: resolve the parent, size
//! the victim(s), record the trash rows, commit under the owner's quota lock,
//! then clean the search index and the path-based tags. The copies had already
//! drifted (v2.1/WebDAV skipped tag cleanup, batch skipped it too, and batch
//! turned every commit error into a 500), so this module owns the sequence and
//! the entrypoints only decide their HTTP shape.

use sea_orm::DatabaseConnection;
use std::collections::HashSet;

use crate::fs::core::RepoHead;
use crate::fs::core::file_ops::FileOps;
use crate::fs::core::trash::{TrashItem, add_batch_to_trash, add_to_trash};
use crate::fs::core::{compute_tree_size_and_files, repo_head, resolve_dir_fs_id};
use crate::repository::Repositories;
use crate::service::fs::quota::QuotaGuard;
use base::error::AppError;
use infra::common::util::join_path;

/// What the caller needs from the repository graph to perform a delete.
pub(crate) struct DeleteDeps<'a> {
    pub db: &'a DatabaseConnection,
    pub repos: &'a Repositories,
    pub indexer: Option<&'a crate::indexer::TextIndexer>,
    pub default_quota: u64,
}

/// How a missing repository/parent directory is reported.
#[derive(Clone, Copy)]
pub(crate) enum ParentPolicy {
    /// A file delete is idempotent: a library with no commits is a success.
    Ok,
    /// A directory delete (and batch deletes) answers 404 for a missing parent.
    NotFound,
}

/// How a missing named entry is reported.
#[derive(Clone, Copy)]
pub(crate) enum EntryPolicy {
    /// A single-file delete is idempotent.
    Ok,
    /// A directory/requested dirent must exist.
    NotFound,
}

/// Which trash API the caller used before the consolidation.
#[derive(Clone, Copy)]
pub(crate) enum TrashMode {
    /// One `add_to_trash` per entry (v1/v2.1 single-entry deletes).
    Single,
    /// One `add_batch_to_trash` (batch deletes).
    Batch,
}

pub(crate) struct DeleteRequest<'a> {
    pub repo_id: &'a str,
    pub parent_dir: &'a str,
    pub names: &'a [String],
    pub email: &'a str,
    pub user_id: i32,
    pub description: String,
    pub parent_policy: ParentPolicy,
    pub entry_policy: EntryPolicy,
    pub trash: TrashMode,
    /// Batch logging records the deleted size and object id; single deletes did not.
    pub activity_detail: bool,
    /// Pre-resolved by batch deletes, which need the ancestor chain for an O(d) commit.
    pub parent_fs_id: Option<String>,
    pub ancestor_chain: Vec<(String, String)>,
}

struct Victim {
    name: String,
    full_path: String,
    is_dir: bool,
    obj_id: String,
    /// The dirent size (0 for directories), which is what the trash row and the
    /// activity detail have always recorded.
    dirent_size: i64,
    /// The quota delta for this entry: the whole subtree for a directory.
    total_size: i64,
    /// File paths below this entry; one entry per file, so the index delete does
    /// not need a second tree walk.
    files: Vec<String>,
}

/// Delete entries below `parent_dir` with one commit and one cleanup pass.
///
/// `names` are dirent names, not paths. A missing name is skipped under
/// [`EntryPolicy::Ok`] (file delete, batch delete) or reported as 404 under
/// [`EntryPolicy::NotFound`] (directory delete).
pub(crate) async fn delete_entries(
    deps: &DeleteDeps<'_>,
    request: DeleteRequest<'_>,
) -> Result<(), AppError> {
    let head = repo_head(deps.repos, request.repo_id).await?;
    if head.head_commit_id.is_none() {
        return match request.parent_policy {
            ParentPolicy::Ok => Ok(()),
            ParentPolicy::NotFound => Err(AppError::NotFound("repo has no commits".into())),
        };
    }
    let root_id = head.root_id.clone();

    let parent_fs_id = match request.parent_fs_id.clone() {
        Some(id) => id,
        None => match resolve_dir_fs_id(deps.repos, request.repo_id, &root_id, request.parent_dir)
            .await
        {
            Ok(id) => id,
            Err(e) => {
                return match request.parent_policy {
                    ParentPolicy::Ok => Ok(()),
                    ParentPolicy::NotFound => Err(crate::service::fs::resolve_parent_error(
                        e,
                        "resolve parent failed",
                    )),
                };
            }
        },
    };

    let parent_data =
        match crate::fs::core::read_fs_dir_data(deps.repos, request.repo_id, &parent_fs_id).await {
            Ok(data) => data,
            Err(e) => {
                return match request.parent_policy {
                    ParentPolicy::Ok => Ok(()),
                    ParentPolicy::NotFound => {
                        Err(AppError::internal(format!("read parent dir failed: {e}")))
                    }
                };
            }
        };

    let mut victims: Vec<Victim> = Vec::new();
    for name in request.names {
        let Some(entry) = parent_data.dirents.iter().find(|d| &d.name == name) else {
            match request.entry_policy {
                EntryPolicy::Ok => continue,
                EntryPolicy::NotFound => {
                    return Err(AppError::NotFound(format!("entry not found: {name}")));
                }
            }
        };
        let is_dir = entry.mode & infra::serialization::S_IFDIR != 0;
        let full_path = join_path(request.parent_dir, name);
        let (total_size, files) = if is_dir {
            let (size, files) =
                compute_tree_size_and_files(deps.repos, request.repo_id, &entry.id, &full_path)
                    .await?;
            (size, files)
        } else {
            (entry.size, vec![full_path.clone()])
        };
        victims.push(Victim {
            name: entry.name.clone(),
            full_path,
            is_dir,
            obj_id: entry.id.clone(),
            dirent_size: entry.size,
            total_size,
            files,
        });
    }

    if victims.is_empty() {
        return Ok(());
    }

    record_trash(deps, &request, &head, &victims).await;

    let total_deleted: i64 = victims.iter().map(|v| v.total_size).sum();
    let names: HashSet<&str> = victims.iter().map(|v| v.name.as_str()).collect();
    let quota = QuotaGuard::begin(deps.repos, request.repo_id, deps.default_quota).await?;
    FileOps::update_dir_tree_and_commit_sized(
        deps.db,
        deps.repos,
        request.repo_id,
        &root_id,
        request.parent_dir,
        &parent_fs_id,
        request.email,
        &request.description,
        &request.ancestor_chain,
        |dirents| {
            dirents.retain(|d| !names.contains(d.name.as_str()));
            Ok(())
        },
        -total_deleted,
        quota.scope(),
        quota.default_quota(),
    )
    .await?;
    drop(quota);

    cleanup_index(deps, request.repo_id, &victims).await;
    cleanup_tags(deps, request.repo_id, &victims).await;
    log_deleted_activity(deps, &request, &victims).await;
    Ok(())
}

/// Record one trash row per victim before the commit removes the dirents.
async fn record_trash(
    deps: &DeleteDeps<'_>,
    request: &DeleteRequest<'_>,
    head: &RepoHead,
    victims: &[Victim],
) {
    let Some(parent_commit_id) = head.head_commit_id.as_deref() else {
        return;
    };
    let items: Vec<TrashItem> = victims
        .iter()
        .map(|victim| TrashItem {
            path: victim.full_path.clone(),
            obj_type: if victim.is_dir { "dir" } else { "file" }.to_string(),
            obj_id: victim.obj_id.clone(),
            obj_name: victim.name.clone(),
            size: victim.dirent_size,
        })
        .collect();
    match request.trash {
        TrashMode::Single => {
            for item in &items {
                if let Err(e) = add_to_trash(
                    deps.repos,
                    request.repo_id,
                    &item.path,
                    &item.obj_type,
                    &item.obj_id,
                    &item.obj_name,
                    item.size,
                    parent_commit_id,
                    request.email,
                )
                .await
                {
                    tracing::warn!("Failed to record trash for {}: {e}", item.path);
                }
            }
        }
        TrashMode::Batch => {
            if let Err(e) = add_batch_to_trash(
                deps.repos,
                request.repo_id,
                items,
                parent_commit_id,
                request.email,
            )
            .await
            {
                tracing::warn!("Failed to record batch trash: {e}");
            }
        }
    }
}

/// Remove the deleted files from the full-text index, one chunk per commit.
async fn cleanup_index(deps: &DeleteDeps<'_>, repo_id: &str, victims: &[Victim]) {
    let Some(indexer) = deps.indexer else {
        return;
    };
    let paths: Vec<String> = victims
        .iter()
        .flat_map(|victim| victim.files.iter().cloned())
        .collect();
    // A directory delete can remove a large subtree; one commit per 4096 paths
    // bounds the index commit size without a second tree walk.
    for chunk in paths.chunks(4096) {
        if let Err(e) = indexer.delete_files(repo_id, chunk).await {
            tracing::warn!("Failed to delete index entries for deleted paths: {e}");
        }
    }
}

/// Remove path-based tags for the deleted entries (and their subtrees).
async fn cleanup_tags(deps: &DeleteDeps<'_>, repo_id: &str, victims: &[Victim]) {
    for victim in victims {
        if let Err(e) = deps
            .repos
            .file_tag
            .delete_by_path_prefix(repo_id, &victim.full_path)
            .await
        {
            tracing::warn!("Failed to delete file tags for {}: {e}", victim.full_path);
        }
        if let Err(e) = deps
            .repos
            .face_tags
            .delete_by_path_prefix(repo_id, &victim.full_path)
            .await
        {
            tracing::warn!(
                "Failed to delete face-tag rows for {}: {e}",
                victim.full_path
            );
        }
    }
}

/// Log one activity row per victim; `delete` op rows aggregate into
/// `batch_delete` inside `log_activity`, which is what the batch endpoint relied on.
async fn log_deleted_activity(
    deps: &DeleteDeps<'_>,
    request: &DeleteRequest<'_>,
    victims: &[Victim],
) {
    for victim in victims {
        let size = request.activity_detail.then_some(victim.dirent_size);
        let obj_id = request.activity_detail.then_some(victim.obj_id.as_str());
        infra::activity_log::log_activity(
            deps.db,
            request.repo_id,
            "delete",
            if victim.is_dir { "dir" } else { "file" },
            &victim.full_path,
            request.user_id,
            None,
            size,
            obj_id,
            None,
            None,
        )
        .await;
    }
}
