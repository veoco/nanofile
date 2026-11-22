//! Keep the full-text index in step with a directory whose path changed.
//!
//! Documents are keyed by `(repo_id, fullpath)`, so every file under a renamed
//! or moved directory needs a document at its new path and its old one dropped.
//!
//! Both trees are walked with their fs ids, which is what pairs a file's old and
//! new path: a path change does not change the content, so the document moves
//! (one add + one delete in the same commit) instead of being dropped and the
//! file extracted again. Anything the index has no document for is *scheduled*
//! (the bounded read passes in the index-files job); documents under paths that
//! no longer exist are dropped. All best effort: the index is derived data, and
//! a failure must never fail the committed rename.

use std::collections::{HashMap, HashSet};

use base::error::AppError;

use crate::indexer::{TextIndexer, collect_file_entries_under};
use crate::repository::Repositories;
use crate::service::index::{IndexScheduler, IndexTarget};

/// Update the index for a directory whose path changed.
///
/// `old_path: None` means it did not exist before (index the new subtree);
/// `new_path: None` means it was deleted (drop the old subtree). `old_root` is
/// only read when `old_path` is set.
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
    let mut new_by_fs_id: HashMap<String, String> = HashMap::new();
    let mut targets: Vec<IndexTarget> = Vec::new();
    if let Some(new_path) = new_path {
        match crate::fs::core::resolve_fs_id(repos, repo_id, new_root, new_path).await {
            Ok(new_fs_id) => {
                match collect_file_entries_under(repos, repo_id, &new_fs_id, new_path).await {
                    Ok(entries) => {
                        for entry in entries {
                            new_files.insert(entry.path.clone());
                            new_by_fs_id.insert(entry.fs_id.clone(), entry.path.clone());
                            targets.push(IndexTarget::new(entry.path, Some(entry.fs_id)));
                        }
                    }
                    Err(e) => tracing::warn!("failed to list files under {new_path}: {e}"),
                }
            }
            Err(e) => tracing::warn!("cannot resolve {new_path} for reindex: {e}"),
        }
    }

    // Pair the old tree with the new one by fs id and move those documents. The
    // move adds before it deletes and both land in one commit, so a reader never
    // finds the file at neither path — the window a delete-then-reindex pair
    // leaves open, and the one the index test exercises.
    let mut stale: Vec<String> = Vec::new();
    if let (Some(old_path), Some(old_root)) = (old_path, old_root) {
        match crate::fs::core::resolve_fs_id(repos, repo_id, old_root, old_path).await {
            Ok(old_fs_id) => {
                match collect_file_entries_under(repos, repo_id, &old_fs_id, old_path).await {
                    Ok(entries) => {
                        let mut moves: Vec<(String, String)> = Vec::new();
                        for entry in entries {
                            match new_by_fs_id.get(&entry.fs_id) {
                                Some(new_path) => {
                                    if entry.path != *new_path {
                                        moves.push((entry.path, new_path.clone()));
                                    }
                                }
                                // Not under the new path: its document is stale.
                                None => {
                                    if !new_files.contains(&entry.path) {
                                        stale.push(entry.path);
                                    }
                                }
                            }
                        }
                        if let Some(indexer) = indexer
                            && !moves.is_empty()
                            && let Err(e) = indexer.move_paths(repo_id, &moves).await
                        {
                            tracing::warn!("failed to move {} index entr(ies): {e}", moves.len());
                        }
                    }
                    Err(e) => {
                        tracing::warn!("failed to list old path {old_path} for index cleanup: {e}")
                    }
                }
            }
            Err(e) => tracing::warn!("cannot resolve old path {old_path} for index cleanup: {e}"),
        }
    }

    // Scheduled before the cleanup below, so a failure there still leaves every
    // file findable. The job skips whatever the move made current at its new
    // path, so renaming an indexed tree reads no block.
    scheduler
        .schedule(repo_id, targets, "dir-path-change")
        .await;

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
