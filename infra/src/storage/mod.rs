//! Blocks live per repository: `{base}/repos/{sha1(repo_id)}/{prefix[..2]}/{block_id}`.
//! Namespacing makes authorization exact: a block is reachable only through a
//! repository the caller belongs to, so an id learned elsewhere is not reachable.
//!
//! Every backend and decorator must preserve two invariants:
//!
//! * A block is stored under the sha1 of its bytes and published atomically; an
//!   interrupted write leaves a `.tmp`, never a short block.
//! * [`BlockStorageBackend::block_size`] equals `read_block(..).len()`; block-map
//!   and `Range` offsets depend on it. A decorator that cannot guarantee this must
//!   return `false` from [`BlockStorageBackend::logical_sizes_are_exact`].
pub mod block_store;
pub mod cdc;
pub mod encrypting_block_store;

use std::sync::Arc;

/// Upper bound on how many leading bytes [`BlockStorageBackend::read_block_prefix`]
/// returns; callers only need a format header.
pub const MAX_BLOCK_PREFIX_LEN: usize = 64 * 1024;

/// Outcome of copying a legacy block during the one-shot layout migration.
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
/// Blocks are identified by their SHA-1 hash and stored in
/// `{base}/repos/{sha1(repo_id)}/{prefix[..2]}/{block_id}`.
#[async_trait::async_trait]
pub trait BlockStorageBackend: Send + Sync + std::fmt::Debug {
    /// Check if a block exists on disk for this repository.
    async fn has_block(&self, repo_id: &str, block_id: &str) -> bool;

    /// Read raw block data by its SHA-1 ID from this repository's directory.
    ///
    /// A store that decrypts transparently returns the *logical* bytes, so
    /// `data.len()` is the size [`Self::block_size`] must agree with.
    async fn read_block(&self, repo_id: &str, block_id: &str) -> Result<Vec<u8>, std::io::Error>;

    /// Read at most `min(max, MAX_BLOCK_PREFIX_LEN)` leading bytes **as stored**,
    /// bypassing any at-rest decryption.
    ///
    /// An encrypting decorator must delegate: the leading bytes are the on-disk format
    /// header. The default reads the whole block and truncates; backends with a real
    /// partial read should override.
    async fn read_block_prefix(
        &self,
        repo_id: &str,
        block_id: &str,
        max: usize,
    ) -> Result<Vec<u8>, std::io::Error> {
        let data = self.read_block(repo_id, block_id).await?;
        let take = max.min(data.len()).min(MAX_BLOCK_PREFIX_LEN);
        Ok(data[..take].to_vec())
    }

    /// Write raw block data, computing and returning its SHA-1 ID.
    async fn write_block(&self, repo_id: &str, data: &[u8]) -> Result<String, std::io::Error>;

    /// Write under a pre-computed SHA-1, skipping the re-hash. Defaults to the hashing
    /// path so backends that cannot take the id for granted still work.
    async fn write_block_with_id(
        &self,
        repo_id: &str,
        _block_id: &str,
        data: &[u8],
    ) -> Result<String, std::io::Error> {
        self.write_block(repo_id, data).await
    }

    /// Like [`Self::write_block_with_id`] but reports whether this call created the
    /// block. Quota cleanup may delete only blocks newly written by the upload —
    /// deduped blocks can be referenced by other files. Implementations should use
    /// exclusive creation (O_CREAT|O_EXCL) so `was_new` is race-free; the default
    /// falls back to the untracked path and always reports `true`.
    async fn write_block_with_id_tracked(
        &self,
        repo_id: &str,
        block_id: &str,
        data: &[u8],
    ) -> Result<(String, bool), std::io::Error> {
        let id = self.write_block_with_id(repo_id, block_id, data).await?;
        Ok((id, true))
    }

    /// Write under a pre-computed id, overwriting any existing block (used by the
    /// lazy→encrypted task to replace plaintext with ciphertext under the same id).
    async fn write_block_with_id_force(
        &self,
        repo_id: &str,
        block_id: &str,
        data: &[u8],
    ) -> Result<String, std::io::Error> {
        self.write_block_with_id(repo_id, block_id, data).await
    }

    /// Convert one legacy plaintext block to ciphertext in place (same id). Returns
    /// `Ok(false)` when it is already encrypted or absent; default no-op.
    async fn convert_legacy_block(
        &self,
        _repo_id: &str,
        _block_id: &str,
    ) -> Result<bool, std::io::Error> {
        Ok(false)
    }

    /// Delete a block file from this repository's directory.
    async fn remove_block(&self, repo_id: &str, block_id: &str) -> Result<(), std::io::Error>;

    /// Logical size of a block; must equal `read_block(repo_id, block_id).len()`.
    async fn block_size(&self, repo_id: &str, block_id: &str) -> Result<i64, std::io::Error>;

    /// Whether [`Self::block_size`] is exact for every block this store holds.
    ///
    /// `range_stream` turns sizes into byte offsets when skipping, so a slightly off
    /// size shifts every byte after it (a size-only check cannot catch it). A decorator
    /// that changes block lengths must return `false`.
    fn logical_sizes_are_exact(&self) -> bool {
        true
    }

    /// Modification time as Unix epoch seconds, or `None` when absent or unreportable.
    ///
    /// Uploads write blocks *before* the commit that references them, so GC uses this
    /// age to avoid deleting a block whose commit predates its reference snapshot;
    /// `None` means eligible, the pre-existing behaviour.
    async fn block_modified_secs(
        &self,
        _repo_id: &str,
        _block_id: &str,
    ) -> Result<Option<i64>, std::io::Error> {
        Ok(None)
    }

    /// List all block IDs stored for one repository.
    async fn list_blocks(&self, repo_id: &str) -> Result<Vec<String>, std::io::Error>;

    /// Visit every block ID of one repository; the default walks [`Self::list_blocks`],
    /// backends may override to stream from the filesystem.
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

    /// Enumerate block IDs through a bounded channel so an async caller can process
    /// each id while the walk runs. [`Self::for_each_block_in_repo`] takes a synchronous
    /// callback, which forces buffering the whole list first. Ids are sent in
    /// enumeration order; the walk stops when the receiver is dropped.
    async fn stream_blocks_in_repo(
        &self,
        repo_id: &str,
        tx: tokio::sync::mpsc::Sender<String>,
    ) -> Result<(), std::io::Error> {
        for id in self.list_blocks(repo_id).await? {
            if tx.send(id).await.is_err() {
                break;
            }
        }
        Ok(())
    }

    /// Repository ids with a block directory on disk, paired with it. Used by GC's
    /// orphan sweep.
    async fn repo_dirs(&self) -> Result<Vec<(String, std::path::PathBuf)>, std::io::Error> {
        Ok(Vec::new())
    }

    /// Drop cached presence results, so a later [`Self::has_block`] re-stats disk (GC
    /// calls this before deleting blocks). No-op for backends without a cache.
    fn invalidate_exists_cache(&self) {}
}

/// Convenience alias for an Arc-wrapped block storage backend.
pub type DynBlockStorage = Arc<dyn BlockStorageBackend>;

/// Create a new filesystem-backed block store at the given directory.
pub fn new_block_store(base_dir: &std::path::Path) -> DynBlockStorage {
    Arc::new(block_store::BlockStorage::new(base_dir.to_path_buf()))
}
