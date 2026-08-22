pub mod block_store;
pub mod cdc;

use std::sync::Arc;

/// Abstract backend for content-addressed block storage.
///
/// Blocks are identified by their SHA-1 hash (40-char hex string) and stored
/// in a two-level directory tree: `{base}/{prefix[..2]}/{block_id}`.
#[async_trait::async_trait]
pub trait BlockStorageBackend: Send + Sync + std::fmt::Debug {
    /// Check if a block exists on disk.
    async fn has_block(&self, block_id: &str) -> bool;

    /// Read raw block data by its SHA-1 ID.
    async fn read_block(&self, block_id: &str) -> Result<Vec<u8>, std::io::Error>;

    /// Write raw block data, computing and returning its SHA-1 ID.
    async fn write_block(&self, data: &[u8]) -> Result<String, std::io::Error>;

    /// Write raw block data under a pre-computed SHA-1 ID, skipping the
    /// re-hash inside `write_block`. Defaults to the hashing path so backends
    /// that cannot take the ID for granted still work.
    async fn write_block_with_id(
        &self,
        _block_id: &str,
        data: &[u8],
    ) -> Result<String, std::io::Error> {
        self.write_block(data).await
    }

    /// Delete a block file from disk.
    async fn remove_block(&self, block_id: &str) -> Result<(), std::io::Error>;

    /// Get the size of a block on disk in bytes.
    async fn block_size(&self, block_id: &str) -> Result<i64, std::io::Error>;

    /// List all block IDs stored on disk.
    async fn list_blocks(&self) -> Result<Vec<String>, std::io::Error>;

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
