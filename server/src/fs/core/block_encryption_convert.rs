//! Background conversion of legacy plaintext blocks to at-rest ciphertext.
//!
//! In `Lazy` encryption mode new writes are encrypted but pre-existing blocks
//! stay plaintext on disk. This task periodically rewrites a bounded batch of
//! legacy plaintext blocks as ciphertext under the same content-addressed id,
//! so the migration window can converge and the server can eventually switch
//! to `On` mode.
//!
//! Block ids are `sha1(logical bytes)` and unchanged by encryption, so Seafile
//! clients, content-addressed dedup and GC are all unaffected. A block is
//! detected as plaintext by probing the GCM-SIV authentication tag (a
//! successful decrypt means it is already ciphertext).
//!
//! To avoid re-probing blocks that are already known to be ciphertext on every
//! run, the caller keeps a `HashSet<[u8; 20]>` of confirmed-encrypted block
//! ids shared across runs; the task skips any id already in the set.

use std::collections::HashSet;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use base::error::AppError;
use infra::storage::DynBlockStorage;

/// Convert up to `batch_limit` legacy plaintext blocks to ciphertext.
///
/// `known_encrypted` holds the raw 20-byte SHA-1 of every block already
/// confirmed to be ciphertext (shared across runs to avoid re-probing). Blocks
/// in the set are skipped without any I/O. Returns the number of blocks
/// converted this run.
pub async fn convert_legacy_blocks(
    block_store: &DynBlockStorage,
    known_encrypted: &Arc<Mutex<HashSet<[u8; 20]>>>,
    batch_limit: usize,
) -> Result<u64, AppError> {
    // 1. Collect candidate ids (bounded by `batch_limit`, skipping known
    //    ciphertext). `for_each_block`'s callback is a synchronous `'static`
    //    `FnMut` and cannot await, so it only collects ids into an Arc-shared
    //    buffer; conversion happens below.
    let candidates = Arc::new(Mutex::new(Vec::<String>::new()));
    let limit = Arc::new(AtomicUsize::new(batch_limit));
    let (candidates_ref, limit_ref, known_ref) =
        (candidates.clone(), limit.clone(), known_encrypted.clone());
    block_store
        .for_each_block(Box::new(move |id| {
            if limit_ref.load(Ordering::Relaxed) == 0 {
                return; // batch limit reached; skip the remaining blocks
            }
            // Skip blocks already confirmed to be ciphertext (zero I/O).
            if known_ref.lock().unwrap().contains(&decode_block_id(id)) {
                return;
            }
            limit_ref.fetch_sub(1, Ordering::Relaxed);
            candidates_ref.lock().unwrap().push(id.to_string());
        }))
        .await?;

    // 2. Probe and convert each candidate (awaited outside the callback so the
    //    runtime is never blocked).
    let candidates = candidates.lock().unwrap().clone();
    let mut converted = 0u64;
    for id in &candidates {
        match block_store.convert_legacy_block(id).await {
            Ok(true) => converted += 1, // plaintext → ciphertext
            Ok(false) => {}             // already ciphertext
            Err(e) => return Err(AppError::internal(e.to_string())),
        }
        // Whether converted or already encrypted, the block is now confirmed
        // ciphertext; remember it so later runs skip it.
        known_encrypted.lock().unwrap().insert(decode_block_id(id));
    }
    Ok(converted)
}

/// Decode a 40-char hex block id to its raw 20-byte SHA-1. `for_each_block`
/// only yields validated 40-hex ids, so this cannot fail; a corrupt id decodes
/// to all-zeroes and is treated as a distinct (never-matching) key.
fn decode_block_id(hex_str: &str) -> [u8; 20] {
    let mut buf = [0u8; 20];
    let _ = hex::decode_to_slice(hex_str, &mut buf);
    buf
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
        let known = Arc::new(Mutex::new(HashSet::new()));

        // Write legacy plaintext straight to the raw store.
        let legacy = b"legacy plaintext block".to_vec();
        let legacy_id = raw.write_block(&legacy).await.unwrap();

        let converted = convert_legacy_blocks(&store, &known, 100).await.unwrap();
        assert_eq!(converted, 1);

        // On-disk bytes are now ciphertext (plaintext + 16-byte tag).
        let on_disk = raw.read_block(&legacy_id).await.unwrap();
        assert_eq!(on_disk.len(), legacy.len() + 16);
        // The id is remembered as ciphertext.
        assert!(known.lock().unwrap().contains(&decode_block_id(&legacy_id)));
    }

    #[tokio::test]
    async fn skips_blocks_already_in_known_set() {
        let (_dir, store, raw) = temp_lazy_store();
        let known = Arc::new(Mutex::new(HashSet::new()));

        let legacy = b"legacy plaintext block".to_vec();
        let legacy_id = raw.write_block(&legacy).await.unwrap();
        // Pre-mark the block as ciphertext so it must be skipped.
        known.lock().unwrap().insert(decode_block_id(&legacy_id));

        let converted = convert_legacy_blocks(&store, &known, 100).await.unwrap();
        assert_eq!(converted, 0);
        // The block was not touched: still plaintext on disk.
        assert_eq!(raw.read_block(&legacy_id).await.unwrap(), legacy);
    }

    #[tokio::test]
    async fn respects_batch_limit() {
        let (_dir, store, raw) = temp_lazy_store();
        let known = Arc::new(Mutex::new(HashSet::new()));

        let mut ids = Vec::new();
        for i in 0..5 {
            let data = format!("legacy block {i}").into_bytes();
            ids.push(raw.write_block(&data).await.unwrap());
        }

        let converted = convert_legacy_blocks(&store, &known, 2).await.unwrap();
        assert_eq!(converted, 2);
        // Only the first two (in traversal order) were converted.
        let mut encrypted_count = 0;
        for id in &ids {
            if raw.read_block(id).await.unwrap().len() > 20 {
                encrypted_count += 1;
            }
        }
        assert_eq!(encrypted_count, 2);
    }

    #[tokio::test]
    async fn idempotent_when_all_blocks_known() {
        let (_dir, store, raw) = temp_lazy_store();
        let known = Arc::new(Mutex::new(HashSet::new()));

        let legacy = b"legacy plaintext block".to_vec();
        let _legacy_id = raw.write_block(&legacy).await.unwrap();

        // First run converts and records the id.
        assert_eq!(convert_legacy_blocks(&store, &known, 100).await.unwrap(), 1);
        // Second run: the id is in the set, so nothing is probed or converted.
        assert_eq!(convert_legacy_blocks(&store, &known, 100).await.unwrap(), 0);
    }
}
