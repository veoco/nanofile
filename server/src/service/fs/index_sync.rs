//! Keep the full-text index in step with a directory whose path changed.
//!
//! A directory rename or move gives every file below it a new path. The index
//! stores documents keyed by `(repo_id, fullpath)`, so each of those files must
//! be re-indexed at its new path and have its old document removed. Doing that
//! one file at a time at each call site is easy to forget — the web directory
//! rename path forgot it entirely, which made every file inside a renamed
//! directory unsearchable — so the work is centralised here.
//!
//! Everything here is best effort: the index is derived data, and failing to
//! update it must never fail the already-committed rename.

use std::collections::HashSet;

use base::error::AppError;
use infra::storage::DynBlockStorage;

use crate::indexer::{TextIndexer, collect_file_paths_under};
use crate::repository::Repositories;

/// Re-index the files under a directory whose path changed.
///
/// * `old_path`: the directory's previous path, or `None` when it did not exist
///   before (which makes this a plain "index the new subtree").
/// * `new_path`: the directory's new path, or `None` when the directory was
///   deleted (which makes this a plain "drop the old subtree").
/// * `old_root` is only read when `old_path` is set.
pub(crate) async fn reindex_dir_change(
    repos: &Repositories,
    indexer: &TextIndexer,
    block_store: &DynBlockStorage,
    repo_id: &str,
    old_path: Option<&str>,
    new_path: Option<&str>,
    old_root: Option<&str>,
    new_root: &str,
) -> Result<(), AppError> {
    // Collect the new subtree's files before touching anything: the walk is
    // also what proves the directory is there.
    let mut new_files: HashSet<String> = HashSet::new();
    if let Some(new_path) = new_path {
        match crate::fs::core::resolve_fs_id(repos, repo_id, new_root, new_path).await {
            Ok(new_fs_id) => {
                match collect_file_paths_under(repos, repo_id, &new_fs_id, new_path).await {
                    Ok(paths) => {
                        new_files = paths.iter().cloned().collect();
                        // Re-index first: if the cleanup below then fails, the
                        // files are still findable at their new path instead of
                        // missing.
                        if let Err(e) = indexer.reindex_files(repo_id, &paths, block_store).await {
                            tracing::warn!("failed to reindex files under {new_path}: {e}");
                        }
                    }
                    Err(e) => tracing::warn!("failed to list files under {new_path}: {e}"),
                }
            }
            Err(e) => tracing::warn!("cannot resolve {new_path} for reindex: {e}"),
        }
    }

    let (Some(old_path), Some(old_root)) = (old_path, old_root) else {
        return Ok(());
    };
    let old_fs_id = match crate::fs::core::resolve_fs_id(repos, repo_id, old_root, old_path).await {
        Ok(id) => id,
        Err(e) => {
            tracing::warn!("cannot resolve old path {old_path} for index cleanup: {e}");
            return Ok(());
        }
    };
    let stale: Vec<String> =
        match collect_file_paths_under(repos, repo_id, &old_fs_id, old_path).await {
            Ok(paths) => paths
                .into_iter()
                // A path present in both trees was just re-indexed above; deleting
                // it here would drop a live document.
                .filter(|path| !new_files.contains(path))
                .collect(),
            Err(e) => {
                tracing::warn!("failed to list old path {old_path} for index cleanup: {e}");
                return Ok(());
            }
        };
    if stale.is_empty() {
        return Ok(());
    }
    if let Err(e) = indexer.delete_files(repo_id, &stale).await {
        tracing::warn!("failed to drop {} stale index entr(ies): {e}", stale.len());
    }
    Ok(())
}

/// Run [`reindex_dir_change`] in the background.
///
/// Re-indexing a subtree re-reads and re-tokenizes every file under it, so the
/// commit request must not wait for it. Mirrors `spawn_reindex` on the sync
/// path; failures are logged and dropped, since the index is derived data.
pub(crate) fn spawn_reindex_dir_change(
    repos: std::sync::Arc<Repositories>,
    indexer: TextIndexer,
    block_store: DynBlockStorage,
    repo_id: String,
    old_path: Option<String>,
    new_path: Option<String>,
    old_root: Option<String>,
    new_root: String,
) {
    tokio::spawn(async move {
        if let Err(e) = reindex_dir_change(
            &repos,
            &indexer,
            &block_store,
            &repo_id,
            old_path.as_deref(),
            new_path.as_deref(),
            old_root.as_deref(),
            &new_root,
        )
        .await
        {
            tracing::warn!("failed to update index for a directory path change: {e}");
        }
    });
}
