//! Cleanup of blocks an upload wrote before its commit succeeded, and the
//! request-scoped guard that schedules it.
//!
//! The guard lives here rather than in the upload handler so WebDAV and the web
//! upload endpoints share one implementation; it holds handles only, so the
//! request can be dropped without keeping the handler module alive.

use std::sync::Arc;

use crate::repository::Repositories;
use crate::service::fs::file::FileService;

/// Delete blocks an upload request wrote but never committed, and drop any quota
/// reservation the request took for them.
///
/// `cleanup_uncommitted_blocks` re-checks whether a live file references each id,
/// so a block committed by a concurrent identical upload survives. Blocks are
/// content-addressed, so deleting an id that a *concurrent* upload wrote but has
/// not committed yet can only make that upload retry (446), never corrupt data.
pub(crate) async fn cleanup_block_ids(
    repos: &Repositories,
    files: &FileService,
    repo_id: &str,
    ids: &[String],
    release_reservations: bool,
) {
    if ids.is_empty() {
        return;
    }
    if release_reservations {
        crate::service::fs::quota::release_blocks(repos, repo_id, ids).await;
    }
    files.cleanup_uncommitted_blocks(repo_id, ids).await;
}

/// Request-scoped cleanup for blocks an upload wrote before its target was
/// validated or committed.
///
/// The upload pipeline streams file parts into the block store before the target
/// path/name is known (the token endpoints receive both in the same multipart
/// body, and a client may send the file part first), and a failure anywhere after
/// that used to leave the blocks unreferenced until the pending-block TTL (48
/// hours by default). An anonymous upload-link holder could therefore fill the
/// data disk with requests that never commit.
///
/// Every upload handler creates this guard right after parsing the body and
/// disarms it only after a successful commit. Dropping it while still armed
/// schedules the cleanup; `tokio::spawn` is deliberate, because `Drop` cannot
/// await. If the runtime is shutting down the spawn may not run, and the durable
/// pending-block ledger is the backstop it was designed to be.
pub(crate) struct UncommittedBlocks {
    repos: Arc<Repositories>,
    files: FileService,
    repo_id: String,
    ids: Vec<String>,
    /// Block-API writes reserve quota per block; streamed uploads do not, so the
    /// guard must not release a reservation it never took (the reservation table
    /// is keyed by block, and a mismatched release would free another upload's).
    release_reservations: bool,
    armed: bool,
}

impl UncommittedBlocks {
    pub(crate) fn new(
        repos: Arc<Repositories>,
        files: FileService,
        repo_id: impl Into<String>,
        ids: Vec<String>,
        release_reservations: bool,
    ) -> Self {
        Self {
            repos,
            files,
            repo_id: repo_id.into(),
            ids,
            release_reservations,
            armed: true,
        }
    }

    /// Add blocks written by a later stage of the same request.
    pub(crate) fn extend(&mut self, ids: impl IntoIterator<Item = String>) {
        self.ids.extend(ids);
    }

    /// The request committed; its blocks are referenced by HEAD now.
    pub(crate) fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for UncommittedBlocks {
    fn drop(&mut self) {
        if !self.armed || self.ids.is_empty() {
            return;
        }
        let repos = self.repos.clone();
        let files = self.files.clone();
        let repo_id = std::mem::take(&mut self.repo_id);
        let ids = std::mem::take(&mut self.ids);
        let release_reservations = self.release_reservations;
        tokio::spawn(async move {
            cleanup_block_ids(&repos, &files, &repo_id, &ids, release_reservations).await;
        });
    }
}
