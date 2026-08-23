use async_trait::async_trait;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use crate::crypto::fs_id::sha1_hex;
use crate::crypto::random_key::{decrypt_block, encrypt_block};
use crate::storage::BlockStorageBackend;

/// Upper bound on the number of block ids held in the existence cache. Blocks
/// are content-addressed and (except under GC) immutable, so a large common
/// block id set is very common; this cap bounds memory when it grows unbounded.
const EXISTS_CACHE_CAPACITY: usize = 100_000;

/// Time-to-live for a cached "block exists" entry. Only blocks removed by GC or
/// `remove_block` stop existing, and GC runs far less often (24h by default),
/// so a short TTL is enough to bound any false-positive window.
const EXISTS_CACHE_TTL: Duration = Duration::from_secs(60);

#[derive(Debug)]
pub struct BlockStorage {
    base_dir: PathBuf,
    /// Guards one-time creation of the 256 prefix directories. Prefix dirs are
    /// static after the first write, so we create them once instead of stat-ing
    /// them on every block write.
    dirs_ready: tokio::sync::OnceCell<()>,
    /// Cache of recently-confirmed-existing block ids → confirmation time.
    /// Content addressing makes blocks immutable, so a presence result never
    /// goes stale except under `remove_block`/GC; the TTL + capacity bound the
    /// worst case. Kept behind a short-lived `Mutex` lock (never held across an
    /// `.await`), so it cannot block the runtime.
    exists_cache: Mutex<HashMap<String, Instant>>,
}

impl BlockStorage {
    pub fn new(base_dir: PathBuf) -> Self {
        Self {
            base_dir,
            dirs_ready: tokio::sync::OnceCell::new(),
            exists_cache: Mutex::new(HashMap::new()),
        }
    }

    /// Drop entries older than [`EXISTS_CACHE_TTL`]. Called under the cache lock;
    /// `elapsed()` (not `duration_since`) so a never-set entry cannot panic.
    fn evict_expired(cache: &mut HashMap<String, Instant>) {
        cache.retain(|_, t| t.elapsed() < EXISTS_CACHE_TTL);
    }

    fn exists_cache_contains(&self, block_id: &str) -> bool {
        let mut cache = self.exists_cache.lock().unwrap();
        Self::evict_expired(&mut cache);
        cache.contains_key(block_id)
    }

    fn exists_cache_insert(&self, block_id: &str) {
        let mut cache = self.exists_cache.lock().unwrap();
        Self::evict_expired(&mut cache);
        if cache.len() >= EXISTS_CACHE_CAPACITY {
            // Capacity reached with all entries still fresh: drop the whole
            // cache. Cheaper than an LRU and only costs a few extra stats.
            cache.clear();
        }
        cache.insert(block_id.to_string(), Instant::now());
    }

    fn exists_cache_remove(&self, block_id: &str) {
        self.exists_cache.lock().unwrap().remove(block_id);
    }

    /// Block IDs are content-addressed SHA-1 hashes: exactly 40 lowercase hex.
    fn is_valid_block_id(block_id: &str) -> bool {
        block_id.len() == 40 && block_id.bytes().all(|b| b.is_ascii_hexdigit())
    }

    fn block_path(&self, block_id: &str) -> PathBuf {
        // Defensive: never index before checking length (callers validate via
        // `is_valid_block_id`, but write_block also routes through here).
        let prefix = block_id.get(..2).unwrap_or(block_id);
        self.base_dir.join(prefix).join(block_id)
    }

    /// Create the 256 prefix directories once, off the async executor.
    /// `OnceCell::get_or_try_init` runs the init future at most once and keeps
    /// the result, so a failure surfaces on this and all subsequent calls.
    async fn ensure_dirs(&self) -> Result<(), std::io::Error> {
        self.dirs_ready
            .get_or_try_init(|| async {
                let base = self.base_dir.clone();
                tokio::task::spawn_blocking(move || -> Result<(), std::io::Error> {
                    for i in 0..=0xFFu16 {
                        std::fs::create_dir_all(base.join(format!("{i:02x}")))?;
                    }
                    Ok(())
                })
                .await
                .map_err(|e| std::io::Error::other(e.to_string()))?
            })
            .await
            .map(|_| ())
    }

    /// Read and decrypt a block.
    pub async fn read_encrypted_block(
        &self,
        block_id: &str,
        file_key: &[u8],
        file_iv: &[u8],
    ) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
        let encrypted = self.read_block(block_id).await?;
        let decrypted = decrypt_block(&encrypted, file_key, file_iv)?;
        Ok(decrypted)
    }

    /// Encrypt data and write as a block.
    pub async fn write_encrypted_block(
        &self,
        data: &[u8],
        file_key: &[u8],
        file_iv: &[u8],
    ) -> Result<String, Box<dyn std::error::Error>> {
        let encrypted = encrypt_block(data, file_key, file_iv);
        let block_id = self.write_block(&encrypted).await?;
        Ok(block_id)
    }
}

#[async_trait]
impl BlockStorageBackend for BlockStorage {
    async fn has_block(&self, block_id: &str) -> bool {
        if !Self::is_valid_block_id(block_id) {
            return false;
        }
        if self.exists_cache_contains(block_id) {
            return true;
        }
        let path = self.block_path(block_id);
        let exists = tokio::fs::try_exists(&path).await.unwrap_or(false);
        if exists {
            self.exists_cache_insert(block_id);
        }
        exists
    }

    async fn read_block(&self, block_id: &str) -> Result<Vec<u8>, std::io::Error> {
        if !Self::is_valid_block_id(block_id) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "invalid block id",
            ));
        }
        tokio::fs::read(self.block_path(block_id)).await
    }

    async fn write_block(&self, data: &[u8]) -> Result<String, std::io::Error> {
        let block_id = sha1_hex(data);
        if self.exists_cache_contains(&block_id) {
            return Ok(block_id);
        }
        let path = self.block_path(&block_id);

        // Content-addressed storage: identical content yields the same SHA-1,
        // so skip the write when the block already exists. Re-uploads and sync
        // retries hit this path constantly.
        if tokio::fs::try_exists(&path).await.unwrap_or(false) {
            self.exists_cache_insert(&block_id);
            return Ok(block_id);
        }

        // Prefix directories are static after the first write; create all 256
        // of them once instead of stat-ing the parent on every block write.
        self.ensure_dirs().await?;

        tokio::fs::write(&path, data).await?;
        self.exists_cache_insert(&block_id);
        Ok(block_id)
    }

    async fn write_block_with_id(
        &self,
        block_id: &str,
        data: &[u8],
    ) -> Result<String, std::io::Error> {
        // The caller has already computed `block_id` as the SHA-1 of `data`
        // (and verified it), so write under it directly without re-hashing.
        let path = self.block_path(block_id);

        // Same dedup semantics as `write_block`.
        if self.exists_cache_contains(block_id) {
            return Ok(block_id.to_string());
        }
        if tokio::fs::try_exists(&path).await.unwrap_or(false) {
            self.exists_cache_insert(block_id);
            return Ok(block_id.to_string());
        }

        self.ensure_dirs().await?;

        tokio::fs::write(&path, data).await?;
        self.exists_cache_insert(block_id);
        Ok(block_id.to_string())
    }

    async fn remove_block(&self, block_id: &str) -> Result<(), std::io::Error> {
        if !Self::is_valid_block_id(block_id) {
            return Ok(());
        }
        let path = self.block_path(block_id);
        if path.exists() {
            tokio::fs::remove_file(&path).await?;
        }
        // Invalidate the cached presence regardless: even if the file was
        // already gone, a stale "exists" entry must not survive a remove.
        self.exists_cache_remove(block_id);
        Ok(())
    }

    fn invalidate_exists_cache(&self) {
        self.exists_cache.lock().unwrap().clear();
    }

    async fn block_size(&self, block_id: &str) -> Result<i64, std::io::Error> {
        if !Self::is_valid_block_id(block_id) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "invalid block id",
            ));
        }
        let path = self.block_path(block_id);
        let size = tokio::fs::metadata(&path).await.map(|m| m.len() as i64)?;
        self.exists_cache_insert(block_id);
        Ok(size)
    }

    async fn list_blocks(&self) -> Result<Vec<String>, std::io::Error> {
        let mut blocks = Vec::new();
        let mut entries = tokio::fs::read_dir(&self.base_dir).await?;

        while let Some(entry) = entries.next_entry().await? {
            if entry.file_type().await?.is_dir() {
                let prefix = entry.file_name();
                let prefix_str = prefix.to_string_lossy();
                if prefix_str.len() == 2 {
                    let mut sub_entries = tokio::fs::read_dir(entry.path()).await?;
                    while let Some(sub_entry) = sub_entries.next_entry().await? {
                        // Only include regular files matching 40-char hex IDs
                        if sub_entry.file_type().await?.is_file()
                            && let Some(name) = sub_entry.file_name().to_str()
                            && name.len() == 40
                            && name.bytes().all(|b| b.is_ascii_hexdigit())
                        {
                            blocks.push(name.to_string());
                        }
                    }
                }
            }
        }

        Ok(blocks)
    }

    /// Stream every block ID directly from the two-level directory layout
    /// without materialising the full list in memory.
    async fn for_each_block(
        &self,
        mut f: Box<dyn for<'a> FnMut(&'a str) + Send>,
    ) -> Result<(), std::io::Error> {
        let mut entries = tokio::fs::read_dir(&self.base_dir).await?;

        while let Some(entry) = entries.next_entry().await? {
            if entry.file_type().await?.is_dir() {
                let prefix = entry.file_name();
                let prefix_str = prefix.to_string_lossy();
                if prefix_str.len() == 2 {
                    let mut sub_entries = tokio::fs::read_dir(entry.path()).await?;
                    while let Some(sub_entry) = sub_entries.next_entry().await? {
                        // Only visit regular files matching 40-char hex IDs
                        if sub_entry.file_type().await?.is_file() {
                            let name = sub_entry.file_name();
                            if let Some(name) = name.to_str()
                                && name.len() == 40
                                && name.bytes().all(|b| b.is_ascii_hexdigit())
                            {
                                f(name);
                            }
                        }
                    }
                }
            }
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    /// Create a unique temp layout
    /// `{tmp}/nf-blockstore-{uuid}/data/blocks` and return `(root, store)`.
    /// The `data` level makes path-traversal ids resolve through real
    /// directories (the kernel rejects `..` through non-existent dirs).
    fn temp_storage() -> (PathBuf, BlockStorage) {
        let root = std::env::temp_dir().join(format!("nf-blockstore-{}", uuid::Uuid::new_v4()));
        let blocks = root.join("data").join("blocks");
        std::fs::create_dir_all(&blocks).unwrap();
        (root, BlockStorage::new(blocks))
    }

    #[tokio::test]
    async fn valid_block_roundtrip_works() {
        let (_root, store) = temp_storage();
        let id = store.write_block(b"hello world").await.unwrap();
        assert_eq!(id.len(), 40);
        assert!(id.bytes().all(|b| b.is_ascii_hexdigit()));
        assert!(store.has_block(&id).await);
        assert_eq!(store.read_block(&id).await.unwrap(), b"hello world");
        assert!(store.block_size(&id).await.unwrap() > 0);
        store.remove_block(&id).await.unwrap();
        assert!(!store.has_block(&id).await);
    }

    /// The streaming visitor must yield exactly the same ids as `list_blocks`.
    #[tokio::test]
    async fn for_each_block_matches_list_blocks() {
        let (_root, store) = temp_storage();
        for i in 0..3 {
            let _ = store
                .write_block(format!("content {i}").as_bytes())
                .await
                .unwrap();
        }

        let mut listed = store.list_blocks().await.unwrap();
        let visited = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let visited_ref = visited.clone();
        store
            .for_each_block(Box::new(move |id| {
                visited_ref.lock().unwrap().push(id.to_string());
            }))
            .await
            .unwrap();
        let mut visited = visited.lock().unwrap().clone();

        listed.sort();
        visited.sort();
        assert_eq!(visited, listed);
        assert_eq!(visited.len(), 3);
    }

    #[tokio::test]
    async fn write_block_dedup_returns_same_id() {
        let (_root, store) = temp_storage();
        // Content-addressed writes are idempotent: re-writing identical content
        // short-circuits and returns the same block id.
        let id1 = store.write_block(b"same content").await.unwrap();
        let id2 = store.write_block(b"same content").await.unwrap();
        assert_eq!(id1, id2);
        assert!(store.has_block(&id1).await);
    }

    #[tokio::test]
    async fn path_traversal_block_ids_are_rejected() {
        let (root, store) = temp_storage();
        // A file living *outside* the block tree (sibling of `blocks/`).
        let secret = root.join("secret.txt");
        tokio::fs::write(&secret, b"secret").await.unwrap();

        // `block_path` is `{base}/{prefix[..2]}/{block_id}`. The prefix
        // segment adds one `..` and the id a second, so
        // `data/blocks/../../secret.txt` resolves to `root/secret.txt`
        // (sibling of the block tree, via real existing dirs).
        let traversal = "../secret.txt";

        // Must not be readable through the block store…
        assert!(store.read_block(traversal).await.is_err());
        assert!(!store.has_block(traversal).await);
        assert!(store.block_size(traversal).await.is_err());
        // …and remove_block must not delete it.
        store.remove_block(traversal).await.unwrap();
        assert_eq!(tokio::fs::read(&secret).await.unwrap(), b"secret");
    }

    #[tokio::test]
    async fn malformed_or_short_block_ids_do_not_panic() {
        let (_root, store) = temp_storage();
        let bad_ids = [
            "",                                         // too short to even take a prefix
            "a",                                        // too short
            "..",                                       // directory traversal
            "nothex",                                   // valid length, not hex
            "abcdef1234567890abcdef1234567890abcdef12", // 40 hex, valid shape but absent
        ];
        for bad in bad_ids {
            assert!(
                !store.has_block(bad).await,
                "has_block({bad:?}) should be false"
            );
            assert!(
                store.read_block(bad).await.is_err(),
                "read_block({bad:?}) should be Err"
            );
            assert!(
                store.block_size(bad).await.is_err(),
                "block_size({bad:?}) should be Err"
            );
        }
    }

    #[tokio::test]
    async fn read_block_rejects_absolute_escape() {
        let (_root, store) = temp_storage();
        // Worst case: a traversal id pointing at a real system file.
        assert!(store.read_block("../../../../etc/passwd").await.is_err());
        assert!(!store.has_block("../../../../etc/passwd").await);
    }

    #[test]
    fn block_path_prefix_uses_first_two_chars() {
        let store = BlockStorage::new(PathBuf::from("/tmp"));
        let p = store.block_path("abcdef");
        assert_eq!(p, Path::new("/tmp/ab/abcdef"));
    }

    /// `remove_block` must invalidate the cached presence entry, not just delete
    /// the file — otherwise a subsequent `has_block` returns a stale "exists".
    #[tokio::test]
    async fn exists_cache_clear_on_remove() {
        let (_root, store) = temp_storage();
        let id = store.write_block(b"cache-me").await.unwrap();
        assert!(store.has_block(&id).await); // populates the existence cache
        store.remove_block(&id).await.unwrap();
        assert!(
            !store.has_block(&id).await,
            "cached presence must be dropped"
        );
    }

    /// `invalidate_exists_cache` drops every cached presence so the next check
    /// re-stats disk instead of trusting a stale entry.
    #[tokio::test]
    async fn invalidate_exists_cache_forces_recheck() {
        let (_root, store) = temp_storage();
        let id = store.write_block(b"cache-me").await.unwrap();
        assert!(store.has_block(&id).await); // populates the existence cache
        store.invalidate_exists_cache();
        store.remove_block(&id).await.unwrap();
        assert!(!store.has_block(&id).await, "recheck after invalidation");
    }
}
