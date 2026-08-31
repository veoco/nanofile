//! A [`BlockStorageBackend`] decorator that transparently encrypts blocks at
//! rest on the server.
//!
//! The wrapper keeps the trait contract ("`read_block` returns the bytes that
//! were written") untouched, so **every** read/write caller — web download &
//! range, sync get/put, indexer, thumbnails, exif, WebDAV, share views,
//! resumable uploads, zip, history — is covered transparently. `GC` only
//! enumerates ids and removes blocks, and storage encryption never changes the
//! content-addressed id, so it is compatible too.
//!
//! Block ids retain their existing semantics: `block_id = sha1(logical bytes)`
//! for plaintext repos and `sha1(CBC ciphertext)` for client-encrypted repos
//! (see [`super::super::crypto::random_key`]). The wrapper never re-hashes the
//! data it writes; the caller has already guaranteed `sha1(data) == block_id`.
//!
//! Three runtime modes ([`BlockEncryptionMode`]):
//! - `Off`: store is used as-is (legacy plaintext).
//! - `On`: everything written is encrypted; reads always decrypt.
//! - `Lazy`: writes are encrypted, reads probe the GCM-SIV tag and fall back to
//!   plaintext on mismatch — the migration window for pre-existing blocks.

use async_trait::async_trait;
use std::io;

use crate::crypto::block_encryption::{BlockCipher, TAG_LEN};
use crate::crypto::fs_id::sha1_hex;
use crate::storage::BlockStorageBackend;
use crate::storage::DynBlockStorage;

/// Runtime at-rest encryption mode. Decided once at startup.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlockEncryptionMode {
    /// Legacy behaviour: store bytes exactly as received (plaintext repos) or
    /// as client-encrypted CBC (sync encrypted repos).
    Off,
    /// Every newly written block is encrypted; every read is decrypted.
    On,
    /// New writes are encrypted; reads accept both encrypted and legacy
    /// plaintext blocks by probing the authentication tag.
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

    /// Encrypt-write `data` under the caller-assigned `id`. Never re-hashes:
    /// the caller has already verified `sha1(data) == id`.
    async fn write_encrypted_with_id(&self, id: &str, data: &[u8]) -> Result<String, io::Error> {
        let ct = self.cipher.encrypt(data);
        self.inner.write_block_with_id(id, &ct).await
    }

    /// Read raw bytes and decrypt them according to `self.mode`.
    ///
    /// `lazy` treats a tag-mismatch as a legacy plaintext block (returns the raw
    /// bytes); `on` treats it as an error.
    async fn read_decrypted(&self, id: &str) -> Result<Vec<u8>, io::Error> {
        let raw = self.inner.read_block(id).await?;
        match self.mode {
            BlockEncryptionMode::Off => Ok(raw),
            BlockEncryptionMode::On => self.cipher.decrypt(&raw).map_err(|_| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    "block at-rest decryption failed",
                )
            }),
            BlockEncryptionMode::Lazy => match self.cipher.decrypt(&raw) {
                Ok(pt) => Ok(pt),
                Err(_) => Ok(raw),
            },
        }
    }
}

#[async_trait]
impl BlockStorageBackend for EncryptingBlockStore {
    async fn has_block(&self, block_id: &str) -> bool {
        self.inner.has_block(block_id).await
    }

    async fn read_block(&self, block_id: &str) -> Result<Vec<u8>, io::Error> {
        self.read_decrypted(block_id).await
    }

    async fn write_block(&self, data: &[u8]) -> Result<String, io::Error> {
        // Content-addressed id is the sha1 of the *logical* bytes, so it is the
        // same whether the store is encrypted or not. We encrypt and write
        // under that id; we do not reuse `write_block` on the inner store, which
        // would key the id on the ciphertext.
        let id = sha1_hex(data);
        self.write_encrypted_with_id(&id, data).await
    }

    async fn write_block_with_id(&self, block_id: &str, data: &[u8]) -> Result<String, io::Error> {
        self.write_encrypted_with_id(block_id, data).await
    }

    async fn remove_block(&self, block_id: &str) -> Result<(), io::Error> {
        self.inner.remove_block(block_id).await
    }

    async fn block_size(&self, block_id: &str) -> Result<i64, io::Error> {
        let stored = self.inner.block_size(block_id).await?;
        match self.mode {
            // Logical size equals the stored size for legacy plaintext blocks.
            BlockEncryptionMode::Off => Ok(stored),
            // GCM-SIV adds exactly the 16-byte tag and no padding, so the
            // logical size is recoverable without reading the block.
            BlockEncryptionMode::On => {
                if stored < TAG_LEN as i64 {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "encrypted block shorter than its authentication tag",
                    ));
                }
                Ok(stored - TAG_LEN as i64)
            }
            // In the migration window we cannot tell ciphertext from plaintext
            // without reading, so probe the tag.
            BlockEncryptionMode::Lazy => {
                let raw = self.inner.read_block(block_id).await?;
                match self.cipher.decrypt(&raw) {
                    Ok(pt) => Ok(pt.len() as i64),
                    Err(_) => Ok(raw.len() as i64),
                }
            }
        }
    }

    async fn list_blocks(&self) -> Result<Vec<String>, io::Error> {
        self.inner.list_blocks().await
    }

    async fn for_each_block(
        &self,
        f: Box<dyn for<'a> FnMut(&'a str) + Send>,
    ) -> Result<(), io::Error> {
        self.inner.for_each_block(f).await
    }

    fn invalidate_exists_cache(&self) {
        self.inner.invalidate_exists_cache();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::block_store::BlockStorage;
    use std::sync::Arc;

    const MASTER: [u8; 32] = [0x42; 32];

    /// Build a raw filesystem store plus a decorator in `mode` over that same
    /// raw store, rooted under a temp dir. Both the raw store and the decorator
    /// share the one underlying directory so tests can write legacy plaintext
    /// via `raw` and inspect on-disk ciphertext through it.
    /// Returns a concrete `Arc<BlockStorage>` backed by a unique temp dir plus
    /// a decorator in `mode` over the very same raw store. The `Arc<BlockStorage>`
    /// derefs to the raw store for inspecting/writing legacy blocks; it also
    /// unsize-coerces to `DynBlockStorage` for the decorator.
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
        let id = store.write_block(data).await.unwrap();

        // Logical id is the sha1 of the plaintext.
        assert_eq!(id, sha1_hex(data));
        // Logical read returns the original bytes.
        assert_eq!(store.read_block(&id).await.unwrap(), data);

        // Underlying bytes differ from the plaintext and hold no plaintext prefix.
        let on_disk = raw.read_block(&id).await.unwrap();
        assert!(!on_disk.starts_with(data));

        // Logical block_size is the plaintext length; physical is +16 bytes.
        assert_eq!(store.block_size(&id).await.unwrap(), data.len() as i64);
        assert_eq!(on_disk.len(), data.len() + TAG_LEN);
        drop(dir);
    }

    #[tokio::test]
    async fn write_block_with_id_preserves_id() {
        let (dir, store, raw) = temp_store(BlockEncryptionMode::On);
        let data = b"caller-verified block";
        let id = sha1_hex(data);
        let returned = store.write_block_with_id(&id, data).await.unwrap();
        assert_eq!(returned, id);
        assert_eq!(store.read_block(&id).await.unwrap(), data);
        // The stored bytes on disk are ciphertext, not the original plaintext.
        assert!(raw.read_block(&id).await.unwrap().len() == data.len() + TAG_LEN);
        drop((dir, raw));
    }

    #[tokio::test]
    async fn deterministic_encryption_same_ciphertext() {
        let (dir, store, raw) = temp_store(BlockEncryptionMode::On);
        let data = b"content-addressed dedup".to_vec();
        let id = store.write_block(&data).await.unwrap();
        let first = raw.read_block(&id).await.unwrap();

        // Remove the deduped block and write the same content again; the
        // produced ciphertext must be byte-identical.
        raw.remove_block(&id).await.unwrap();
        raw.invalidate_exists_cache();
        let id2 = store.write_block(&data).await.unwrap();
        assert_eq!(id2, id);
        assert_eq!(raw.read_block(&id).await.unwrap(), first);
        drop(dir);
    }

    #[tokio::test]
    async fn lazy_mode_reads_both_legacy_and_encrypted() {
        let (dir, store, raw) = temp_store(BlockEncryptionMode::Lazy);
        // Legacy plaintext written straight to the raw store.
        let legacy = b"pre-existing plaintext block".to_vec();
        let legacy_id = raw.write_block(&legacy).await.unwrap();
        // New encrypted block via the decorator.
        let fresh = b"freshly encrypted block".to_vec();
        let fresh_id = store.write_block(&fresh).await.unwrap();

        assert_eq!(store.read_block(&legacy_id).await.unwrap(), legacy);
        assert_eq!(store.read_block(&fresh_id).await.unwrap(), fresh);
        assert_eq!(
            store.block_size(&legacy_id).await.unwrap(),
            legacy.len() as i64
        );
        assert_eq!(
            store.block_size(&fresh_id).await.unwrap(),
            fresh.len() as i64
        );
        drop(dir);
    }

    #[tokio::test]
    async fn on_mode_tamper_detected() {
        let (dir, store, raw) = temp_store(BlockEncryptionMode::On);
        let data = b"tamper me".to_vec();
        let id = store.write_block(&data).await.unwrap();

        // Corrupt one byte on disk.
        let mut ct = raw.read_block(&id).await.unwrap();
        ct[0] ^= 0xFF;
        raw.remove_block(&id).await.unwrap();
        raw.write_block_with_id(&id, &ct).await.unwrap();

        // In `on` mode a tampered block is a hard read error.
        assert!(store.read_block(&id).await.is_err());
        drop((dir, store));
    }

    #[tokio::test]
    async fn lazy_mode_untampered_reads_plaintext_and_encrypted() {
        let (dir, store, _raw) = temp_store(BlockEncryptionMode::Lazy);
        let data = b"lazy plaintext block".to_vec();
        let id = store.write_block(&data).await.unwrap();
        assert_eq!(store.read_block(&id).await.unwrap(), data);
        drop(dir);
    }

    #[tokio::test]
    async fn on_mode_short_block_size_is_error() {
        let (dir, _store, raw) = temp_store(BlockEncryptionMode::Off);
        let short = b"tiny".to_vec();
        let id = raw.write_block(&short).await.unwrap();
        raw.invalidate_exists_cache();

        let on_store = EncryptingBlockStore::new(
            raw,
            BlockCipher::from_master_key(&MASTER),
            BlockEncryptionMode::On,
        );
        assert!(on_store.block_size(&id).await.is_err());
        drop(dir);
    }
}
