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

use crate::crypto::block_encryption::{BlockCipher, HEADER_LEN, TAG_LEN};
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

    /// Encrypt-write `data` under the caller-assigned `id`, in `repo_id`'s
    /// directory. Never re-hashes: the caller has already verified
    /// `sha1(data) == id`.
    async fn write_encrypted_with_id(
        &self,
        repo_id: &str,
        id: &str,
        data: &[u8],
    ) -> Result<String, io::Error> {
        let ct = self.encrypt_offload(data.to_vec()).await?;
        self.inner.write_block_with_id(repo_id, id, &ct).await
    }

    /// Read raw bytes and decrypt them according to `self.mode`.
    ///
    /// `lazy` treats a tag-mismatch as a legacy plaintext block (returns the raw
    /// bytes); `on` treats it as an error.
    async fn read_decrypted(&self, repo_id: &str, id: &str) -> Result<Vec<u8>, io::Error> {
        let raw = self.inner.read_block(repo_id, id).await?;
        self.decrypt_offload(raw).await
    }

    /// Encrypt `data` on the blocking thread pool. AES-GCM-SIV is CPU-bound, so
    /// running it inline on the async executor would stall other tasks sharing
    /// the worker thread.
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
            // During the migration window a block may still be pre-encryption
            // plaintext. Decide that by **format**, not by a decryption
            // failure: a block that carries the `NFE1` header and then fails
            // authentication is corrupt or tampered with, and returning its
            // bytes verbatim would defeat the integrity guarantee entirely.
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

    async fn write_block(&self, repo_id: &str, data: &[u8]) -> Result<String, io::Error> {
        // Content-addressed id is the sha1 of the *logical* bytes, so it is the
        // same whether the store is encrypted or not. We encrypt and write
        // under that id; we do not reuse `write_block` on the inner store, which
        // would key the id on the ciphertext.
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
        // Encrypt the logical bytes, then delegate to inner's tracked write
        // so the `was_new` flag reflects whether the ciphertext file was
        // created on disk.
        let ct = self.encrypt_offload(data.to_vec()).await?;
        self.inner
            .write_block_with_id_tracked(repo_id, block_id, &ct)
            .await
    }

    async fn remove_block(&self, repo_id: &str, block_id: &str) -> Result<(), io::Error> {
        self.inner.remove_block(repo_id, block_id).await
    }

    async fn block_size(&self, repo_id: &str, block_id: &str) -> Result<i64, io::Error> {
        let stored = self.inner.block_size(repo_id, block_id).await?;
        match self.mode {
            // Logical size equals the stored size for legacy plaintext blocks.
            BlockEncryptionMode::Off => Ok(stored),
            // GCM-SIV adds exactly the 16-byte tag and no padding, plus the
            // 6-byte `NFE1 || key_id` header, so the logical size is
            // recoverable without reading the block.
            BlockEncryptionMode::On => {
                let overhead = (TAG_LEN + HEADER_LEN) as i64;
                if stored < overhead {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "encrypted block shorter than its header and authentication tag",
                    ));
                }
                Ok(stored - overhead)
            }
            // In the migration window a block may still be plaintext; decide by
            // the `NFE1` header rather than by a failed decode.
            BlockEncryptionMode::Lazy => {
                let raw = self.inner.read_block(repo_id, block_id).await?;
                if !BlockCipher::looks_encrypted(&raw) {
                    return Ok(raw.len() as i64);
                }
                let cipher = self.cipher.clone();
                tokio::task::spawn_blocking(move || {
                    cipher.decrypt(&raw).map(|pt| pt.len() as i64).map_err(|_| {
                        io::Error::new(
                            io::ErrorKind::InvalidData,
                            "block at-rest decryption failed",
                        )
                    })
                })
                .await
                .map_err(|e| io::Error::other(e.to_string()))?
            }
        }
    }

    async fn convert_legacy_block(&self, repo_id: &str, block_id: &str) -> Result<bool, io::Error> {
        // Read the raw on-disk bytes (not through `read_decrypted`, which in
        // `Lazy` mode would fall back to plaintext and hide the distinction).
        let raw = self.inner.read_block(repo_id, block_id).await?;
        // Probe the GCM-SIV tag: a successful decrypt means the block is already
        // ciphertext (nothing to do); a tag mismatch means legacy plaintext.
        // Both the probe and the re-encryption are CPU-bound, so run them on the
        // blocking thread pool.
        let cipher = self.cipher.clone();
        let converted = tokio::task::spawn_blocking(move || {
            // Only a block without the versioned header can be legacy
            // plaintext. A block that has the header but fails to authenticate
            // must not be re-encrypted blind — that would launder a corrupt or
            // tampered block into a "valid" one.
            if BlockCipher::looks_encrypted(&raw) {
                return None;
            }
            Some(cipher.encrypt(&raw))
        })
        .await
        .map_err(|e| io::Error::other(e.to_string()))?;
        if let Some(ct) = converted {
            self.inner
                .write_block_with_id_force(repo_id, block_id, &ct)
                .await?;
            Ok(true)
        } else {
            Ok(false)
        }
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
    use crate::storage::block_store::BlockStorage;
    use std::sync::Arc;

    const MASTER: [u8; 32] = [0x42; 32];
    const REPO: &str = "cfcab3e0-9eb4-4c4f-92d0-87db2cd8290d";

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
        let id = store.write_block(REPO, data).await.unwrap();

        // Logical id is the sha1 of the plaintext.
        assert_eq!(id, sha1_hex(data));
        // Logical read returns the original bytes.
        assert_eq!(store.read_block(REPO, &id).await.unwrap(), data);

        // Underlying bytes differ from the plaintext and hold no plaintext prefix.
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
        // The stored bytes on disk are ciphertext, not the original plaintext.
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

        // Remove the deduped block and write the same content again; the
        // produced ciphertext must be byte-identical.
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
        // Legacy plaintext written straight to the raw store.
        let legacy = b"pre-existing plaintext block".to_vec();
        let legacy_id = raw.write_block(REPO, &legacy).await.unwrap();
        // New encrypted block via the decorator.
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

        // Corrupt one byte on disk.
        let mut ct = raw.read_block(REPO, &id).await.unwrap();
        ct[0] ^= 0xFF;
        raw.remove_block(REPO, &id).await.unwrap();
        raw.write_block_with_id(REPO, &id, &ct).await.unwrap();

        // In `on` mode a tampered block is a hard read error.
        assert!(store.read_block(REPO, &id).await.is_err());
        drop((dir, store));
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
}
