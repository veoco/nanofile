//! One-shot conversion of legacy plaintext blocks to at-rest ciphertext.
//!
//! In `Lazy` encryption mode new writes are encrypted but pre-existing blocks
//! stay plaintext on disk. This task rewrites every legacy plaintext block as
//! ciphertext under the same content-addressed id, so the migration window can
//! converge and the server can eventually switch to `On` mode.
//!
//! Block ids are `sha1(logical bytes)` and unchanged by encryption, so Seafile
//! clients, content-addressed dedup and GC are all unaffected. A block is
//! detected as plaintext by probing the GCM-SIV authentication tag (a
//! successful decrypt means it is already ciphertext).
//!
//! The conversion is idempotent and blocks are immutable (except under GC), so
//! the set of legacy plaintext blocks is fixed and finite. The task therefore
//! runs once: it walks every block, probes each exactly once, converts the
//! plaintext ones, then writes a marker file in `block_dir` recording
//! completion. Later runs (startup or manual re-trigger) see the marker and
//! skip entirely with zero I/O. No in-memory set is needed because each block
//! is probed only once.

use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use base::error::AppError;
use infra::storage::DynBlockStorage;

/// One-shot converter for legacy plaintext blocks.
pub struct BlockEncryptionConverter;

impl BlockEncryptionConverter {
    /// Marker file name, stored directly in `block_dir`.
    const MARKER_FILE: &'static str = ".encryption_converted";

    /// Whether the conversion has already completed (marker file exists).
    pub async fn is_converted(block_dir: &Path) -> bool {
        tokio::fs::try_exists(block_dir.join(Self::MARKER_FILE))
            .await
            .unwrap_or(false)
    }

    /// Convert every legacy plaintext block to ciphertext, in bounded batches
    /// with a short sleep between batches to keep load smooth. Returns the
    /// number of blocks converted.
    pub async fn convert_legacy_blocks(
        block_store: &DynBlockStorage,
        batch_limit: usize,
        batch_sleep: Duration,
    ) -> Result<u64, AppError> {
        // 1. Collect all block ids. `for_each_block`'s callback is a synchronous
        //    `'static` `FnMut` and cannot await, so it only collects ids into an
        //    Arc-shared buffer; conversion happens below.
        let ids = Arc::new(Mutex::new(Vec::<String>::new()));
        let ids_ref = ids.clone();
        block_store
            .for_each_block(Box::new(move |id| {
                ids_ref.lock().unwrap().push(id.to_string())
            }))
            .await?;
        let ids = ids.lock().unwrap().clone();

        // 2. Probe and convert in batches, sleeping between batches so a large
        //    store never hogs CPU/IO in one burst.
        let mut converted = 0u64;
        for chunk in ids.chunks(batch_limit) {
            for id in chunk {
                match block_store.convert_legacy_block(id).await {
                    Ok(true) => converted += 1, // plaintext → ciphertext
                    Ok(false) => {}             // already ciphertext
                    Err(e) => return Err(AppError::internal(e.to_string())),
                }
            }
            if chunk.len() == batch_limit {
                tokio::time::sleep(batch_sleep).await;
            }
        }
        Ok(converted)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use infra::crypto::block_encryption::BlockCipher;
    use infra::storage::BlockStorageBackend;
    use infra::storage::block_store::BlockStorage;
    use infra::storage::encrypting_block_store::{BlockEncryptionMode, EncryptingBlockStore};

    const MASTER: [u8; 32] = [0x42; 32];

    /// Build a lazy-mode decorator over a raw store sharing one temp dir, so
    /// tests can write legacy plaintext via the raw store and inspect on-disk
    /// ciphertext through it.
    fn temp_lazy_store() -> (tempfile::TempDir, DynBlockStorage, Arc<BlockStorage>) {
        let dir = tempfile::tempdir().unwrap();
        let blocks = dir.path().join("data").join("blocks");
        std::fs::create_dir_all(&blocks).unwrap();
        let raw = Arc::new(BlockStorage::new(blocks));
        let decorator: DynBlockStorage = Arc::new(EncryptingBlockStore::new(
            raw.clone(),
            BlockCipher::from_master_key(&MASTER),
            BlockEncryptionMode::Lazy,
        ));
        (dir, decorator, raw)
    }

    #[tokio::test]
    async fn converts_plaintext_blocks_to_ciphertext() {
        let (_dir, store, raw) = temp_lazy_store();

        // Write legacy plaintext straight to the raw store.
        let legacy = b"legacy plaintext block".to_vec();
        let legacy_id = raw.write_block(&legacy).await.unwrap();

        let converted =
            BlockEncryptionConverter::convert_legacy_blocks(&store, 100, Duration::ZERO)
                .await
                .unwrap();
        assert_eq!(converted, 1);

        // On-disk bytes are now ciphertext (plaintext + 16-byte tag).
        let on_disk = raw.read_block(&legacy_id).await.unwrap();
        assert_eq!(on_disk.len(), legacy.len() + 16);
    }

    #[tokio::test]
    async fn skips_blocks_already_encrypted() {
        let (_dir, store, raw) = temp_lazy_store();

        // A freshly written block via the decorator is already ciphertext.
        let fresh = b"freshly encrypted block".to_vec();
        let fresh_id = store.write_block(&fresh).await.unwrap();
        // A legacy plaintext block.
        let legacy = b"legacy plaintext block".to_vec();
        let legacy_id = raw.write_block(&legacy).await.unwrap();

        let converted =
            BlockEncryptionConverter::convert_legacy_blocks(&store, 100, Duration::ZERO)
                .await
                .unwrap();
        // Only the plaintext block is counted as converted.
        assert_eq!(converted, 1);
        // The already-encrypted block is untouched (still decryptable).
        assert_eq!(store.read_block(&fresh_id).await.unwrap(), fresh);
        // The legacy block is now ciphertext.
        assert_eq!(
            raw.read_block(&legacy_id).await.unwrap().len(),
            legacy.len() + 16
        );
    }

    #[tokio::test]
    async fn is_converted_reflects_marker_file() {
        let dir = tempfile::tempdir().unwrap();
        let block_dir = dir.path().join("data").join("blocks");
        std::fs::create_dir_all(&block_dir).unwrap();

        assert!(!BlockEncryptionConverter::is_converted(&block_dir).await);
        tokio::fs::write(block_dir.join(".encryption_converted"), b"done")
            .await
            .unwrap();
        assert!(BlockEncryptionConverter::is_converted(&block_dir).await);
    }
}
