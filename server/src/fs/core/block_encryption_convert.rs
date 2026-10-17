//! One-shot conversion of legacy blocks to versioned at-rest ciphertext.
//!
//! In `Lazy` mode new writes are encrypted but pre-existing blocks stay as
//! they are: either pre-encryption **plaintext**, or header-less **ciphertext**
//! written before the versioned (`NFE1 || key_id || ciphertext || tag`) header
//! existed. Both must converge before the server switches to `On`.
//!
//! A block is classified by probing the GCM-SIV tag: a body that decrypts is
//! ciphertext (re-encrypted, header only); anything else is plaintext and is
//! encrypted as-is. Block ids are `sha1(logical bytes)` and unchanged, so
//! Seafile clients, dedup and GC are unaffected. Idempotent, and a marker in
//! `block_dir` makes later runs skip with zero I/O.

use std::path::Path;
use std::time::Duration;

use base::error::AppError;
use infra::storage::DynBlockStorage;

pub struct BlockEncryptionConverter;

impl BlockEncryptionConverter {
    /// Marker file name, stored directly in `block_dir`.
    const MARKER_FILE: &'static str = ".encryption_converted";

    pub async fn is_converted(block_dir: &Path) -> bool {
        tokio::fs::try_exists(block_dir.join(Self::MARKER_FILE))
            .await
            .unwrap_or(false)
    }

    /// Convert every legacy plaintext block, in bounded batches with a short
    /// sleep between them. Returns the number converted.
    ///
    /// Block ids stream from the store through a bounded channel and are
    /// converted as they arrive, so the walk never holds a whole library's id
    /// list in memory and cannot run far ahead of the conversion.
    pub async fn convert_legacy_blocks(
        block_store: &DynBlockStorage,
        batch_limit: usize,
        batch_sleep: Duration,
    ) -> Result<u64, AppError> {
        let batch_limit = batch_limit.max(1);
        let repo_ids: Vec<String> = block_store
            .repo_dirs()
            .await?
            .into_iter()
            .map(|(repo_id, _dir)| repo_id)
            .collect();

        let mut converted = 0u64;
        for repo_id in repo_ids {
            // The walk is a separate task: the callback-style enumeration cannot
            // await the per-block conversion, so the bounded channel keeps pace.
            let (tx, mut rx) = tokio::sync::mpsc::channel::<String>(batch_limit * 2);
            let store = block_store.clone();
            let walk_repo = repo_id.clone();
            let walker =
                tokio::spawn(async move { store.stream_blocks_in_repo(&walk_repo, tx).await });

            let mut since_sleep = 0usize;
            while let Some(id) = rx.recv().await {
                match block_store.convert_legacy_block(&repo_id, &id).await {
                    Ok(true) => converted += 1, // plaintext → ciphertext
                    Ok(false) => {}             // already ciphertext
                    Err(e) => return Err(AppError::internal(e.to_string())),
                }
                since_sleep += 1;
                if since_sleep == batch_limit {
                    since_sleep = 0;
                    tokio::time::sleep(batch_sleep).await;
                }
            }

            walker
                .await
                .map_err(|e| AppError::internal(format!("block walk task failed: {e}")))?
                .map_err(|e| AppError::internal(e.to_string()))?;
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
    use std::sync::Arc;

    const MASTER: [u8; 32] = [0x42; 32];
    const REPO: &str = "cfcab3e0-9eb4-4c4f-92d0-87db2cd8290d";

    /// A lazy-mode decorator over a raw store sharing one temp dir, so tests can
    /// write legacy plaintext via the raw store and inspect on-disk ciphertext.
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
        let legacy_id = raw.write_block(REPO, &legacy).await.unwrap();

        let converted =
            BlockEncryptionConverter::convert_legacy_blocks(&store, 100, Duration::ZERO)
                .await
                .unwrap();
        assert_eq!(converted, 1);

        // On-disk bytes are now `NFE1 || key_id || ciphertext || tag`.
        let on_disk = raw.read_block(REPO, &legacy_id).await.unwrap();
        assert_eq!(on_disk.len(), legacy.len() + 16 + 6);
    }

    #[tokio::test]
    async fn skips_blocks_already_encrypted() {
        let (_dir, store, raw) = temp_lazy_store();

        let fresh = b"freshly encrypted block".to_vec();
        let fresh_id = store.write_block(REPO, &fresh).await.unwrap();
        let legacy = b"legacy plaintext block".to_vec();
        let legacy_id = raw.write_block(REPO, &legacy).await.unwrap();

        let converted =
            BlockEncryptionConverter::convert_legacy_blocks(&store, 100, Duration::ZERO)
                .await
                .unwrap();
        // Only the plaintext block is counted as converted.
        assert_eq!(converted, 1);
        // The already-encrypted block is untouched (still decryptable).
        assert_eq!(store.read_block(REPO, &fresh_id).await.unwrap(), fresh);
        assert_eq!(
            raw.read_block(REPO, &legacy_id).await.unwrap().len(),
            legacy.len() + 16 + 6
        );
    }

    #[tokio::test]
    async fn converts_headerless_legacy_ciphertext_without_changing_content() {
        let (_dir, store, raw) = temp_lazy_store();

        // Versioned ciphertext from before the header: 6-byte header stripped.
        let data = b"headerless legacy ciphertext block".to_vec();
        let id = store.write_block(REPO, &data).await.unwrap();
        let stored = raw.read_block(REPO, &id).await.unwrap();
        assert_eq!(&stored[..4], b"NFE1", "fixture must start versioned");
        raw.write_block_with_id_force(REPO, &id, &stored[6..])
            .await
            .unwrap();
        raw.invalidate_exists_cache();

        let converted =
            BlockEncryptionConverter::convert_legacy_blocks(&store, 100, Duration::ZERO)
                .await
                .unwrap();
        assert_eq!(
            converted, 1,
            "a header-less ciphertext block needs conversion"
        );

        // Format change only: content and id unchanged; the versioned header is
        // back.
        assert_eq!(store.read_block(REPO, &id).await.unwrap(), data);
        let on_disk = raw.read_block(REPO, &id).await.unwrap();
        assert_eq!(&on_disk[..4], b"NFE1");
        assert_eq!(on_disk.len(), data.len() + 16 + 6);
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
