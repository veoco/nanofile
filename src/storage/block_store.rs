use async_trait::async_trait;
use std::path::PathBuf;

use crate::crypto::fs_id::compute_block_id;
use crate::crypto::random_key::{decrypt_block, encrypt_block};
use crate::storage::BlockStorageBackend;

pub struct BlockStorage {
    base_dir: PathBuf,
}

impl BlockStorage {
    pub fn new(base_dir: PathBuf) -> Self {
        Self { base_dir }
    }

    fn block_path(&self, block_id: &str) -> PathBuf {
        let prefix = &block_id[..2];
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
        self.block_path(block_id).exists()
    }

    async fn read_block(&self, block_id: &str) -> Result<Vec<u8>, std::io::Error> {
        tokio::fs::read(self.block_path(block_id)).await
    }

    async fn write_block(&self, data: &[u8]) -> Result<String, std::io::Error> {
        let block_id = compute_block_id(data);
        let path = self.block_path(&block_id);

        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }

        tokio::fs::write(&path, data).await?;
        Ok(block_id)
    }

    async fn remove_block(&self, block_id: &str) -> Result<(), std::io::Error> {
        let path = self.block_path(block_id);
        if path.exists() {
            tokio::fs::remove_file(&path).await?;
        }
        Ok(())
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
                        if let Some(name) = sub_entry.file_name().to_str() {
                            blocks.push(name.to_string());
                        }
                    }
                }
            }
        }

        Ok(blocks)
    }
}
