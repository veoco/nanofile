//! Block storage.
//!
//! Blocks live in a **per-repository** directory tree:
//! `{base}/repos/{sha1(repo_id)}/{prefix[..2]}/{block_id}`.
//!
//! Namespacing by repository is what makes authorization exact: a request can
//! only name a block through a repository the caller is a member of, and the
//! path is derived from that repository, so a block id learned elsewhere (a
//! cached `fs_object`, a revoked collaborator's client state) is simply not
//! reachable. The official server and the official clients both organize blocks
//! per library for the same reason.
//!
//! Every read/write method therefore takes a `repo_id`; enumeration is
//! per-repository as well.
pub mod block_store;
pub mod cdc;
pub mod encrypting_block_store;

use std::sync::Arc;

/// Outcome of copying one legacy block into a repository's directory during the
/// one-shot layout migration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LegacyCopyOutcome {
    /// The block was copied into the repository's directory.
    Copied,
    /// The destination already held a block of the same size (idempotent re-run).
    AlreadyPresent,
    /// The legacy store does not contain this block id.
    MissingSource,
}

/// Abstract backend for content-addressed block storage.
///
/// Blocks are identified by their SHA-1 hash (40-char hex string) and stored in
/// a three-level directory tree:
/// `{base}/repos/{sha1(repo_id)}/{prefix[..2]}/{block_id}`.
#[async_trait::async_trait]
pub trait BlockStorageBackend: Send + Sync + std::fmt::Debug {
    /// Check if a block exists on disk for this repository.
    async fn has_block(&self, repo_id: &str, block_id: &str) -> bool;

    /// Read raw block data by its SHA-1 ID from this repository's directory.
    async fn read_block(&self, repo_id: &str, block_id: &str) -> Result<Vec<u8>, std::io::Error>;

    /// Write raw block data, computing and returning its SHA-1 ID.
    async fn write_block(&self, repo_id: &str, data: &[u8]) -> Result<String, std::io::Error>;

    /// Write raw block data under a pre-computed SHA-1 ID, skipping the
    /// re-hash inside `write_block`. Defaults to the hashing path so backends
    /// that cannot take the ID for granted still work.
    async fn write_block_with_id(
        &self,
        repo_id: &str,
        _block_id: &str,
        data: &[u8],
    ) -> Result<String, std::io::Error> {
        self.write_block(repo_id, data).await
    }

    /// Like [`Self::write_block_with_id`] but also reports whether this call
    /// created the block on disk (`was_new`). Used by the upload quota-cleanup
    /// path: only blocks newly written by the current upload are safe to delete
    /// on quota failure — deduped blocks may be referenced by other files.
    ///
    /// Implementations should use exclusive creation (O_CREAT|O_EXCL) so the
    /// `was_new` flag is race-free under concurrent writes of the same block.
    /// The default falls back to the untracked path and always reports `true`.
    async fn write_block_with_id_tracked(
        &self,
        repo_id: &str,
        block_id: &str,
        data: &[u8],
    ) -> Result<(String, bool), std::io::Error> {
        let id = self.write_block_with_id(repo_id, block_id, data).await?;
        Ok((id, true))
    }

    /// Write raw block data under a pre-computed ID, overwriting any existing
    /// block. Unlike [`Self::write_block_with_id`], this never short-circuits
    /// on an existing block. Used by the lazy→encrypted conversion task to
    /// rewrite a legacy plaintext block as ciphertext under the same ID.
    async fn write_block_with_id_force(
        &self,
        repo_id: &str,
        block_id: &str,
        data: &[u8],
    ) -> Result<String, std::io::Error> {
        self.write_block_with_id(repo_id, block_id, data).await
    }

    /// Convert one legacy plaintext block to ciphertext in place (same ID).
    /// Returns `Ok(true)` if the block was converted, `Ok(false)` if it was
    /// already encrypted or absent. Default no-op for backends without
    /// at-rest encryption.
    async fn convert_legacy_block(
        &self,
        _repo_id: &str,
        _block_id: &str,
    ) -> Result<bool, std::io::Error> {
        Ok(false)
    }

    /// Delete a block file from this repository's directory.
    async fn remove_block(&self, repo_id: &str, block_id: &str) -> Result<(), std::io::Error>;

    /// Get the size of a block on disk in bytes.
    async fn block_size(&self, repo_id: &str, block_id: &str) -> Result<i64, std::io::Error>;

    /// List all block IDs stored for one repository.
    async fn list_blocks(&self, repo_id: &str) -> Result<Vec<String>, std::io::Error>;

    /// Visit every block ID stored for one repository without necessarily
    /// materialising the full list. The default walks [`Self::list_blocks`];
    /// backends may override to stream directly from the filesystem.
    async fn for_each_block_in_repo(
        &self,
        repo_id: &str,
        mut f: Box<dyn for<'a> FnMut(&'a str) + Send>,
    ) -> Result<(), std::io::Error> {
        let ids = self.list_blocks(repo_id).await?;
        for id in &ids {
            f(id);
        }
        Ok(())
    }

    /// Repository ids that currently have a block directory on disk, paired with
    /// that directory. Used by GC's orphan sweep (a repo whose directory is
    /// still on disk but which no longer has a row in the database).
    async fn repo_dirs(&self) -> Result<Vec<(String, std::path::PathBuf)>, std::io::Error> {
        Ok(Vec::new())
    }

    /// Drop any cached "block exists" results. Called before a batch of blocks
    /// is deleted (e.g. by GC) so a later [`Self::has_block`] re-stats disk
    /// instead of trusting a stale presence entry. No-op for backends that keep
    /// no presence cache.
    fn invalidate_exists_cache(&self) {}
}

/// Convenience alias for an Arc-wrapped block storage backend.
pub type DynBlockStorage = Arc<dyn BlockStorageBackend>;

/// Create a new filesystem-backed block store at the given directory.
pub fn new_block_store(base_dir: &std::path::Path) -> DynBlockStorage {
    Arc::new(block_store::BlockStorage::new(base_dir.to_path_buf()))
}
