//! Keep the full-text index in step with a directory whose path changed.
//!
//! Documents are keyed by `(repo_id, fullpath)`, so every file under a renamed
//! or moved directory needs a document at its new path and its old one dropped.
//!
//! The new subtree is *scheduled* (the bounded read passes in the index-files
//! job) and stale documents are dropped synchronously. All best effort: the
//! index is derived data, and a failure must never fail the committed rename.

use std::collections::HashSet;

use base::error::AppError;

use crate::indexer::{TextIndexer, collect_file_entries_under};
use crate::repository::Repositories;
use crate::service::index::{IndexScheduler, IndexTarget};

/// Update the index for a directory whose path changed.
///
/// `old_path: None` means it did not exist before (index the new subtree);
/// `new_path: None` means it was deleted (drop the old subtree). `old_root` is
/// only read when `old_path` is set.
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
    // Collect the new subtree first: the walk also proves the directory exists.
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
                        // Schedule before the cleanup below: if that fails, the
                        // files are still findable at their new path.
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
                // Scheduled above; deleting a path present in both trees would
                // drop a live document.
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
