//! At-rest encryption decorator for [`BlockStorageBackend`].
//!
//! The wrapper keeps the trait contract (`read_block` returns the bytes that were
//! written), so every caller is covered; GC only enumerates ids and removes blocks,
//! and the content-addressed id never changes. Ids keep their semantics
//! (`sha1(logical bytes)`, or `sha1(CBC ciphertext)` for client-encrypted repos).
//!
//! Modes: `Off` (plaintext), `On` (encrypt writes, decrypt reads), `Lazy` (writes
//! encrypted, header-less reads fall back to plaintext during migration).
//! Ciphertext is either the versioned `NFE1 || key_id` header plus tag or a
//! header-less legacy tag. `decrypt` accepts both, and reported sizes must derive
//! from the *stored* format or blocks under-report by six bytes.

use async_trait::async_trait;
use std::io;

use crate::crypto::block_encryption::{BlockCipher, HEADER_LEN};
use crate::crypto::fs_id::sha1_hex;
use crate::storage::BlockStorageBackend;
use crate::storage::DynBlockStorage;

/// Runtime at-rest encryption mode. Decided once at startup.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlockEncryptionMode {
    /// Legacy: store bytes as received (plaintext, or client-encrypted CBC).
    Off,
    /// Every newly written block is encrypted; every read is decrypted.
    On,
    /// New writes encrypted; reads accept both encrypted and legacy plaintext.
    Lazy,
}

/// Wraps an inner block store with server-side at-rest encryption.
pub struct EncryptingBlockStore {
    inner: DynBlockStorage,
    cipher: BlockCipher,
    mode: BlockEncryptionMode,
}

impl std::fmt::Debug for EncryptingBlockStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Never render the key material (`BlockCipher::Debug` is keyless).
        f.debug_struct("EncryptingBlockStore")
            .field("inner", &self.inner)
            .field("mode", &self.mode)
            .finish_non_exhaustive()
    }
}

impl EncryptingBlockStore {
    /// Wrap `inner` in an at-rest-encrypting store.
    pub fn new(inner: DynBlockStorage, cipher: BlockCipher, mode: BlockEncryptionMode) -> Self {
        Self {
            inner,
            cipher,
            mode,
        }
    }

    /// Encrypt-write `data` under the caller-assigned `id`; never re-hashes, since the
    /// caller verified `sha1(data) == id`.
    async fn write_encrypted_with_id(
        &self,
        repo_id: &str,
        id: &str,
        data: &[u8],
    ) -> Result<String, io::Error> {
        let ct = self.encrypt_offload(data.to_vec()).await?;
        self.inner.write_block_with_id(repo_id, id, &ct).await
    }

    /// Read raw bytes and decrypt per `self.mode`: `lazy` treats a tag mismatch as a
    /// legacy plaintext block, `on` treats it as an error.
    async fn read_decrypted(&self, repo_id: &str, id: &str) -> Result<Vec<u8>, io::Error> {
        let raw = self.inner.read_block(repo_id, id).await?;
        self.decrypt_offload(raw).await
    }

    /// Encrypt on the blocking pool: AES-GCM-SIV is CPU-bound and would stall the
    /// async worker.
    async fn encrypt_offload(&self, data: Vec<u8>) -> Result<Vec<u8>, io::Error> {
        let cipher = self.cipher.clone();
        tokio::task::spawn_blocking(move || cipher.encrypt(&data))
            .await
            .map_err(|e| io::Error::other(e.to_string()))
    }

    /// Decrypt `raw` on the blocking thread pool, honoring `self.mode`.
    async fn decrypt_offload(&self, raw: Vec<u8>) -> Result<Vec<u8>, io::Error> {
        let cipher = self.cipher.clone();
        let mode = self.mode;
        tokio::task::spawn_blocking(move || match mode {
            BlockEncryptionMode::Off => Ok(raw),
            BlockEncryptionMode::On => cipher.decrypt(&raw).map_err(|_| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    "block at-rest decryption failed",
                )
            }),
            // During migration a block may still be plaintext. Decide by **format**, not by a
            // decryption failure: a block carrying `NFE1` that then fails authentication is
            // corrupt, and returning its bytes would defeat the integrity guarantee.
            BlockEncryptionMode::Lazy => {
                if !BlockCipher::looks_encrypted(&raw) {
                    return Ok(raw);
                }
                cipher.decrypt(&raw).map_err(|_| {
                    io::Error::new(
                        io::ErrorKind::InvalidData,
                        "block at-rest decryption failed",
                    )
                })
            }
        })
        .await
        .map_err(|e| io::Error::other(e.to_string()))?
    }
}

#[async_trait]
impl BlockStorageBackend for EncryptingBlockStore {
    async fn has_block(&self, repo_id: &str, block_id: &str) -> bool {
        self.inner.has_block(repo_id, block_id).await
    }

    async fn read_block(&self, repo_id: &str, block_id: &str) -> Result<Vec<u8>, io::Error> {
        self.read_decrypted(repo_id, block_id).await
    }

    async fn read_block_prefix(
        &self,
        repo_id: &str,
        block_id: &str,
        max: usize,
    ) -> Result<Vec<u8>, io::Error> {
        // The leading bytes are the *stored* header, so do not go through `read_decrypted`.
        self.inner.read_block_prefix(repo_id, block_id, max).await
    }

    async fn write_block(&self, repo_id: &str, data: &[u8]) -> Result<String, io::Error> {
        // The id is the sha1 of the *logical* bytes, so encrypt and write under it;
        // delegating to inner `write_block` would key the id on the ciphertext.
        let id = sha1_hex(data);
        self.write_encrypted_with_id(repo_id, &id, data).await
    }

    async fn write_block_with_id(
        &self,
        repo_id: &str,
        block_id: &str,
        data: &[u8],
    ) -> Result<String, io::Error> {
        self.write_encrypted_with_id(repo_id, block_id, data).await
    }

    async fn write_block_with_id_tracked(
        &self,
        repo_id: &str,
        block_id: &str,
        data: &[u8],
    ) -> Result<(String, bool), io::Error> {
        // Encrypt first, then delegate the tracked write so `was_new` reflects whether the
        // ciphertext file was created.
        let ct = self.encrypt_offload(data.to_vec()).await?;
        self.inner
            .write_block_with_id_tracked(repo_id, block_id, &ct)
            .await
    }

    async fn write_block_with_id_force(
        &self,
        repo_id: &str,
        block_id: &str,
        data: &[u8],
    ) -> Result<String, io::Error> {
        // `data` is plaintext, so encrypt and force-write the ciphertext: the trait default
        // would re-enter `write_block_with_id`, which skips an existing block.
        let ct = self.encrypt_offload(data.to_vec()).await?;
        self.inner
            .write_block_with_id_force(repo_id, block_id, &ct)
            .await
    }

    async fn remove_block(&self, repo_id: &str, block_id: &str) -> Result<(), io::Error> {
        self.inner.remove_block(repo_id, block_id).await
    }

    /// Logical size of a stored block: must equal `read_block(..).len()`, because clients
    /// derive range/segmented offsets from it and `range_stream` skips by it, so a
    /// systematic error shifts everything after it.
    async fn block_size(&self, repo_id: &str, block_id: &str) -> Result<i64, io::Error> {
        let stored = self.inner.block_size(repo_id, block_id).await?;
        match self.mode {
            // Logical size equals the stored size for legacy plaintext blocks.
            BlockEncryptionMode::Off => Ok(stored),
            // GCM-SIV adds no padding, but the stored overhead differs: versioned ciphertext
            // carries the 6-byte `NFE1 || key_id` header and the 16-byte tag, while older
            // blocks carry only the tag. Decide by the stored header, not the constant.
            BlockEncryptionMode::On => {
                let prefix = self
                    .inner
                    .read_block_prefix(repo_id, block_id, HEADER_LEN)
                    .await?;
                let stored_len = usize::try_from(stored).map_err(|_| {
                    io::Error::new(io::ErrorKind::InvalidData, "negative stored block size")
                })?;
                let logical = BlockCipher::plaintext_len(&prefix, stored_len).map_err(|_| {
                    io::Error::new(
                        io::ErrorKind::InvalidData,
                        "encrypted block shorter than its header and authentication tag",
                    )
                })?;
                i64::try_from(logical)
                    .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "block size overflow"))
            }
            // In the migration window a block may still be plaintext; decide by the `NFE1`
            // header. A header-less block is handed back verbatim, so its logical size is the
            // stored size — the two must agree.
            BlockEncryptionMode::Lazy => {
                // Only the header is needed: a versioned block is ciphertext by
                // construction, and `/block-sizes` asks for this on every block of a
                // listing. Reading and decrypting the whole block for its length was
                // the most expensive metadata query in the migration window.
                let prefix = self
                    .inner
                    .read_block_prefix(repo_id, block_id, HEADER_LEN)
                    .await?;
                if !BlockCipher::looks_encrypted(&prefix) {
                    return Ok(stored);
                }
                let stored_len = usize::try_from(stored).map_err(|_| {
                    io::Error::new(io::ErrorKind::InvalidData, "negative stored block size")
                })?;
                let logical = BlockCipher::plaintext_len(&prefix, stored_len).map_err(|_| {
                    io::Error::new(
                        io::ErrorKind::InvalidData,
                        "encrypted block shorter than its header and authentication tag",
                    )
                })?;
                i64::try_from(logical)
                    .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "block size overflow"))
            }
        }
    }

    /// The stored file's age is unaffected by encryption: header and tag are written in
    /// the same `create`.
    async fn block_modified_secs(
        &self,
        repo_id: &str,
        block_id: &str,
    ) -> Result<Option<i64>, io::Error> {
        self.inner.block_modified_secs(repo_id, block_id).await
    }

    async fn stream_blocks_in_repo(
        &self,
        repo_id: &str,
        tx: tokio::sync::mpsc::Sender<String>,
    ) -> Result<(), io::Error> {
        self.inner.stream_blocks_in_repo(repo_id, tx).await
    }

    async fn convert_legacy_block(&self, repo_id: &str, block_id: &str) -> Result<bool, io::Error> {
        // Read raw on-disk bytes, not through `read_decrypted` (which would hide plaintext).
        let raw = self.inner.read_block(repo_id, block_id).await?;
        // A versioned block is already converted — or corrupt. Re-encrypting would launder
        // the corruption into a "valid" block.
        if BlockCipher::looks_encrypted(&raw) {
            return Ok(false);
        }
        // Header-less: either legacy *plaintext* or legacy *ciphertext*. Only the tag probe
        // can tell them apart, and it decides what to re-encrypt: a body that decrypts is
        // ciphertext, so its plaintext gets re-encrypted (a format change, not content);
        // anything else is plaintext and is encrypted as-is. Encrypting raw ciphertext
        // would double-encrypt it.
        let cipher = self.cipher.clone();
        let ct = tokio::task::spawn_blocking(move || {
            let plaintext = cipher.decrypt(&raw).unwrap_or(raw);
            cipher.encrypt(&plaintext)
        })
        .await
        .map_err(|e| io::Error::other(e.to_string()))?;
        self.inner
            .write_block_with_id_force(repo_id, block_id, &ct)
            .await?;
        Ok(true)
    }

    async fn list_blocks(&self, repo_id: &str) -> Result<Vec<String>, io::Error> {
        self.inner.list_blocks(repo_id).await
    }

    async fn for_each_block_in_repo(
        &self,
        repo_id: &str,
        f: Box<dyn for<'a> FnMut(&'a str) + Send>,
    ) -> Result<(), io::Error> {
        self.inner.for_each_block_in_repo(repo_id, f).await
    }

    async fn repo_dirs(&self) -> Result<Vec<(String, std::path::PathBuf)>, io::Error> {
        self.inner.repo_dirs().await
    }

    fn invalidate_exists_cache(&self) {
        self.inner.invalidate_exists_cache();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::crypto::block_encryption::{MAGIC, TAG_LEN};
    use crate::storage::block_store::BlockStorage;
    use std::sync::Arc;

    const MASTER: [u8; 32] = [0x42; 32];
    const REPO: &str = "cfcab3e0-9eb4-4c4f-92d0-87db2cd8290d";

    /// Build a raw store plus a decorator in `mode` over it, under a temp dir, so tests
    /// can write legacy plaintext through `raw` and inspect on-disk ciphertext. Returns
    /// the concrete `Arc<BlockStorage>` (which derefs to the raw store and unsize-
    /// coerces to `DynBlockStorage`).
    fn temp_store(
        mode: BlockEncryptionMode,
    ) -> (tempfile::TempDir, EncryptingBlockStore, Arc<BlockStorage>) {
        let dir = tempfile::tempdir().unwrap();
        let blocks = dir.path().join("data").join("blocks");
        std::fs::create_dir_all(&blocks).unwrap();
        let raw = Arc::new(BlockStorage::new(blocks));
        let decorator =
            EncryptingBlockStore::new(raw.clone(), BlockCipher::from_master_key(&MASTER), mode);
        (dir, decorator, raw)
    }

    #[tokio::test]
    async fn on_mode_roundtrip_and_ciphertext_on_disk() {
        let (dir, store, raw) = temp_store(BlockEncryptionMode::On);
        let data = b"secret plaintext block";
        let id = store.write_block(REPO, data).await.unwrap();

        assert_eq!(id, sha1_hex(data));
        assert_eq!(store.read_block(REPO, &id).await.unwrap(), data);

        let on_disk = raw.read_block(REPO, &id).await.unwrap();
        assert!(!on_disk.starts_with(data));

        // Logical block_size is the plaintext length; physical is +16 bytes.
        assert_eq!(
            store.block_size(REPO, &id).await.unwrap(),
            data.len() as i64
        );
        assert_eq!(on_disk.len(), data.len() + TAG_LEN + HEADER_LEN);
        drop(dir);
    }

    #[tokio::test]
    async fn write_block_with_id_preserves_id() {
        let (dir, store, raw) = temp_store(BlockEncryptionMode::On);
        let data = b"caller-verified block";
        let id = sha1_hex(data);
        let returned = store.write_block_with_id(REPO, &id, data).await.unwrap();
        assert_eq!(returned, id);
        assert_eq!(store.read_block(REPO, &id).await.unwrap(), data);
        assert!(
            raw.read_block(REPO, &id).await.unwrap().len() == data.len() + TAG_LEN + HEADER_LEN
        );
        drop((dir, raw));
    }

    #[tokio::test]
    async fn deterministic_encryption_same_ciphertext() {
        let (dir, store, raw) = temp_store(BlockEncryptionMode::On);
        let data = b"content-addressed dedup".to_vec();
        let id = store.write_block(REPO, &data).await.unwrap();
        let first = raw.read_block(REPO, &id).await.unwrap();

        // Removing the deduped block and rewriting the same content must reproduce the
        // same ciphertext.
        raw.remove_block(REPO, &id).await.unwrap();
        raw.invalidate_exists_cache();
        let id2 = store.write_block(REPO, &data).await.unwrap();
        assert_eq!(id2, id);
        assert_eq!(raw.read_block(REPO, &id).await.unwrap(), first);
        drop(dir);
    }

    #[tokio::test]
    async fn lazy_mode_reads_both_legacy_and_encrypted() {
        let (dir, store, raw) = temp_store(BlockEncryptionMode::Lazy);
        let legacy = b"pre-existing plaintext block".to_vec();
        let legacy_id = raw.write_block(REPO, &legacy).await.unwrap();
        let fresh = b"freshly encrypted block".to_vec();
        let fresh_id = store.write_block(REPO, &fresh).await.unwrap();

        assert_eq!(store.read_block(REPO, &legacy_id).await.unwrap(), legacy);
        assert_eq!(store.read_block(REPO, &fresh_id).await.unwrap(), fresh);
        assert_eq!(
            store.block_size(REPO, &legacy_id).await.unwrap(),
            legacy.len() as i64
        );
        assert_eq!(
            store.block_size(REPO, &fresh_id).await.unwrap(),
            fresh.len() as i64
        );
        drop(dir);
    }

    #[tokio::test]
    async fn on_mode_tamper_detected() {
        let (dir, store, raw) = temp_store(BlockEncryptionMode::On);
        let data = b"tamper me".to_vec();
        let id = store.write_block(REPO, &data).await.unwrap();

        let mut ct = raw.read_block(REPO, &id).await.unwrap();
        ct[0] ^= 0xFF;
        raw.remove_block(REPO, &id).await.unwrap();
        raw.write_block_with_id(REPO, &id, &ct).await.unwrap();

        // In `on` mode a tampered block is a hard read error.
        assert!(store.read_block(REPO, &id).await.is_err());
        drop((dir, store));
    }

    /// Sizing a versioned block must not decrypt it: the header and the stored
    /// length already determine the logical size. A corrupted body makes a read
    /// fail, which proves the size came from the header and not the plaintext.
    #[tokio::test]
    async fn lazy_block_size_does_not_decrypt_versioned_blocks() {
        let (dir, store, raw) = temp_store(BlockEncryptionMode::Lazy);
        let data = b"size comes from the header".to_vec();
        let id = store.write_block(REPO, &data).await.unwrap();

        let mut ciphertext = raw.read_block(REPO, &id).await.unwrap();
        let last = ciphertext.len() - 1;
        ciphertext[last] ^= 0xFF;
        raw.remove_block(REPO, &id).await.unwrap();
        raw.write_block_with_id(REPO, &id, &ciphertext)
            .await
            .unwrap();

        assert!(
            store.read_block(REPO, &id).await.is_err(),
            "the corrupted body must fail authentication"
        );
        assert_eq!(
            store.block_size(REPO, &id).await.unwrap(),
            data.len() as i64
        );
        drop(dir);
    }

    #[tokio::test]
    async fn lazy_mode_untampered_reads_plaintext_and_encrypted() {
        let (dir, store, _raw) = temp_store(BlockEncryptionMode::Lazy);
        let data = b"lazy plaintext block".to_vec();
        let id = store.write_block(REPO, &data).await.unwrap();
        assert_eq!(store.read_block(REPO, &id).await.unwrap(), data);
        drop(dir);
    }

    #[tokio::test]
    async fn on_mode_short_block_size_is_error() {
        let (dir, _store, raw) = temp_store(BlockEncryptionMode::Off);
        let short = b"tiny".to_vec();
        let id = raw.write_block(REPO, &short).await.unwrap();
        raw.invalidate_exists_cache();

        let on_store = EncryptingBlockStore::new(
            raw,
            BlockCipher::from_master_key(&MASTER),
            BlockEncryptionMode::On,
        );
        assert!(on_store.block_size(REPO, &id).await.is_err());
        drop(dir);
    }

    /// Deterministic pseudo-random bytes (no `rand`); `seed` keeps blocks distinct so
    /// dedup cannot collapse the fixture.
    fn pseudo_block(seed: u64, len: usize) -> Vec<u8> {
        let mut x: u64 = 0x9E37_79B9_7F4A_7C15 ^ seed.wrapping_mul(0x9E37_79B9);
        (0..len)
            .map(|_| {
                x = x
                    .wrapping_mul(6364136223846793005)
                    .wrapping_add(1442695040888963407);
                (x >> 33) as u8
            })
            .collect()
    }

    /// Rewrite a stored block in the pre-`NFE1` header-less format: the same GCM-SIV
    /// body without the 6-byte header.
    async fn strip_versioned_header(raw: &BlockStorage, id: &str) {
        let stored = raw.read_block(REPO, id).await.unwrap();
        assert_eq!(&stored[..4], MAGIC.as_slice(), "fixture must be versioned");
        raw.write_block_with_id_force(REPO, id, &stored[HEADER_LEN..])
            .await
            .unwrap();
        raw.invalidate_exists_cache();
    }

    #[tokio::test]
    async fn on_mode_sizes_headerless_legacy_blocks_by_the_tag_only() {
        let (dir, store, raw) = temp_store(BlockEncryptionMode::On);
        let data = pseudo_block(1, 4096);
        let id = store.write_block(REPO, &data).await.unwrap();
        strip_versioned_header(&raw, &id).await;

        // Still readable: `decrypt` accepts the header-less legacy format.
        assert_eq!(store.read_block(REPO, &id).await.unwrap(), data);
        // Sizing follows the *stored* format; subtracting the versioned overhead
        // under-reported legacy blocks by 6 bytes and shifted segmented downloads.
        assert_eq!(
            store.block_size(REPO, &id).await.unwrap(),
            data.len() as i64
        );
        drop(dir);
    }

    /// The property the incident violated: block-list sizes must sum to the logical file
    /// size for mixed versioned and legacy blocks, and equal `read_block(..).len()`.
    #[tokio::test]
    async fn on_mode_reported_sizes_sum_to_the_logical_file_size_for_mixed_formats() {
        let (dir, store, raw) = temp_store(BlockEncryptionMode::On);
        let plaintexts: Vec<Vec<u8>> = [4096usize, 8192, 8192, 2048]
            .iter()
            .enumerate()
            .map(|(seed, len)| pseudo_block(seed as u64 + 1, *len))
            .collect();
        let mut ids = Vec::new();
        for data in &plaintexts {
            ids.push(store.write_block(REPO, data).await.unwrap());
        }
        // The first three blocks predate the versioned header (three skipped legacy blocks
        // produced an 18-byte gap); the last was written by the current build.
        for id in &ids[..3] {
            strip_versioned_header(&raw, id).await;
        }

        let mut total = 0i64;
        for (id, data) in ids.iter().zip(&plaintexts) {
            let size = store.block_size(REPO, id).await.unwrap();
            assert_eq!(
                size as usize,
                store.read_block(REPO, id).await.unwrap().len(),
                "block_size must equal read_block(..).len()"
            );
            assert_eq!(size, data.len() as i64);
            total += size;
        }
        assert_eq!(
            total,
            plaintexts.iter().map(|d| d.len() as i64).sum::<i64>()
        );
        drop(dir);
    }

    /// A header-less *ciphertext* block converted to the versioned format keeps its
    /// plaintext; encrypting the on-disk bytes would double-encrypt and serve garbage.
    #[tokio::test]
    async fn convert_legacy_ciphertext_keeps_the_plaintext_and_adds_the_header() {
        let (dir, store, raw) = temp_store(BlockEncryptionMode::On);
        let data = pseudo_block(11, 1024);
        let id = store.write_block(REPO, &data).await.unwrap();
        strip_versioned_header(&raw, &id).await;
        assert_eq!(
            raw.read_block(REPO, &id).await.unwrap().len(),
            data.len() + TAG_LEN
        );

        assert!(store.convert_legacy_block(REPO, &id).await.unwrap());
        // Format change only: same plaintext, same id, versioned bytes on disk.
        assert_eq!(store.read_block(REPO, &id).await.unwrap(), data);
        let stored = raw.read_block(REPO, &id).await.unwrap();
        assert_eq!(&stored[..4], MAGIC.as_slice());
        assert_eq!(stored.len(), data.len() + HEADER_LEN + TAG_LEN);
        // Idempotent: an already-versioned block is left alone.
        assert!(!store.convert_legacy_block(REPO, &id).await.unwrap());
        drop(dir);
    }

    /// Legacy *plaintext* is encrypted exactly once.
    #[tokio::test]
    async fn convert_legacy_plaintext_encrypts_it_once() {
        let (dir, store, raw) = temp_store(BlockEncryptionMode::On);
        let data = pseudo_block(13, 512);
        let id = raw.write_block(REPO, &data).await.unwrap();
        raw.invalidate_exists_cache();

        assert!(store.convert_legacy_block(REPO, &id).await.unwrap());
        assert_eq!(store.read_block(REPO, &id).await.unwrap(), data);
        let stored = raw.read_block(REPO, &id).await.unwrap();
        assert_eq!(&stored[..4], MAGIC.as_slice());
        assert_eq!(stored.len(), data.len() + HEADER_LEN + TAG_LEN);
        drop(dir);
    }
}
