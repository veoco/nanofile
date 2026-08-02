use async_trait::async_trait;
use std::path::PathBuf;

use crate::crypto::fs_id::sha1_hex;
use crate::crypto::random_key::{decrypt_block, encrypt_block};
use crate::storage::BlockStorageBackend;

#[derive(Debug)]
pub struct BlockStorage {
    base_dir: PathBuf,
}

impl BlockStorage {
    pub fn new(base_dir: PathBuf) -> Self {
        Self { base_dir }
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
        let path = self.block_path(block_id);
        tokio::fs::try_exists(&path).await.unwrap_or(false)
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
        let path = self.block_path(&block_id);

        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }

        tokio::fs::write(&path, data).await?;
        Ok(block_id)
    }

    async fn remove_block(&self, block_id: &str) -> Result<(), std::io::Error> {
        if !Self::is_valid_block_id(block_id) {
            return Ok(());
        }
        let path = self.block_path(block_id);
        if path.exists() {
            tokio::fs::remove_file(&path).await?;
        }
        Ok(())
    }

    async fn block_size(&self, block_id: &str) -> Result<i64, std::io::Error> {
        if !Self::is_valid_block_id(block_id) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "invalid block id",
            ));
        }
        let path = self.block_path(block_id);
        tokio::fs::metadata(&path).await.map(|m| m.len() as i64)
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
}
