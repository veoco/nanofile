pub mod block_store;
pub mod cdc;

use std::sync::Arc;

pub use crate::permission::lock::{check_commit_file_locks, upsert_lock_timestamp};
pub use crate::permission::repo::{check_repo_read_permission, check_repo_write_permission};

/// Abstract backend for content-addressed block storage.
///
/// Blocks are identified by their SHA-1 hash (40-char hex string) and stored
/// in a two-level directory tree: `{base}/{prefix[..2]}/{block_id}`.
#[async_trait::async_trait]
pub trait BlockStorageBackend: Send + Sync {
    /// Check if a block exists on disk.
    async fn has_block(&self, block_id: &str) -> bool;

    /// Read raw block data by its SHA-1 ID.
    async fn read_block(&self, block_id: &str) -> Result<Vec<u8>, std::io::Error>;

    /// Write raw block data, computing and returning its SHA-1 ID.
    async fn write_block(&self, data: &[u8]) -> Result<String, std::io::Error>;

    /// Delete a block file from disk.
    async fn remove_block(&self, block_id: &str) -> Result<(), std::io::Error>;

    /// List all block IDs stored on disk.
    async fn list_blocks(&self) -> Result<Vec<String>, std::io::Error>;
}

/// Convenience alias for an Arc-wrapped block storage backend.
pub type DynBlockStorage = Arc<dyn BlockStorageBackend>;

/// Create a new filesystem-backed block store at the given directory.
pub fn new_block_store(base_dir: &std::path::Path) -> DynBlockStorage {
    Arc::new(block_store::BlockStorage::new(base_dir.to_path_buf()))
}
