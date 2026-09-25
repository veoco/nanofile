//! Keep the full-text index in step with a directory whose path changed.
//!
//! A directory rename or move gives every file below it a new path. The index
//! stores documents keyed by `(repo_id, fullpath)`, so each of those files must
//! get a document at its new path and have its old document removed. Doing that
//! one file at a time at each call site is easy to forget — the web directory
//! rename path forgot it entirely, which made every file inside a renamed
//! directory unsearchable — so the work is centralised here.
//!
//! The new subtree is *scheduled*, not read here: it may be thousands of files,
//! and the index-files job is where the bounded read-and-tokenize pass lives.
//! Removing the old documents stays synchronous because it is cheap (a delete
//! query per path, no content read).
//!
//! Everything here is best effort: the index is derived data, and failing to
//! update it must never fail the already-committed rename.

use std::collections::HashSet;

use base::error::AppError;

use crate::indexer::{TextIndexer, collect_file_entries_under};
use crate::repository::Repositories;
use crate::service::index::{IndexScheduler, IndexTarget};

/// Update the index for a directory whose path changed.
///
/// * `old_path`: the directory's previous path, or `None` when it did not exist
///   before (which makes this a plain "index the new subtree").
/// * `new_path`: the directory's new path, or `None` when the directory was
///   deleted (which makes this a plain "drop the old subtree").
/// * `old_root` is only read when `old_path` is set.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn reindex_dir_change(
    repos: &Repositories,
    indexer: Option<&TextIndexer>,
    scheduler: &IndexScheduler,
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
                match collect_file_entries_under(repos, repo_id, &new_fs_id, new_path).await {
                    Ok(entries) => {
                        let targets: Vec<IndexTarget> = entries
                            .into_iter()
                            .map(|entry| {
                                new_files.insert(entry.path.clone());
                                IndexTarget::new(entry.path, Some(entry.fs_id))
                            })
                            .collect();
                        // Schedule first: if the cleanup below then fails, the
                        // files are still findable at their new path instead of
                        // missing.
                        scheduler
                            .schedule(repo_id, targets, "dir-path-change")
                            .await;
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
        match collect_file_entries_under(repos, repo_id, &old_fs_id, old_path).await {
            Ok(entries) => entries
                .into_iter()
                .map(|entry| entry.path)
                // A path present in both trees was just scheduled above;
                // deleting it here would drop a live document.
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
    let Some(indexer) = indexer else {
        return Ok(());
    };
    if let Err(e) = indexer.delete_files(repo_id, &stale).await {
        tracing::warn!("failed to drop {} stale index entr(ies): {e}", stale.len());
    }
    Ok(())
}
