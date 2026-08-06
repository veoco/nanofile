use crate::repository::Repositories;
use base::common::FsFileData;
use base::error::AppError;
use futures::{Stream, StreamExt};
use infra::crypto::random_key::decrypt_block;
use infra::storage::DynBlockStorage;

pub struct Downloader;

impl Downloader {
    pub async fn download_file(
        repos: &Repositories,
        repo_id: &str,
        path: &str,
        block_store: &DynBlockStorage,
        // Optional decryption key (key, iv) — when set, blocks are decrypted
        // after reading. Used for encrypted repos during web download.
        dec_key: Option<(&[u8], &[u8])>,
    ) -> Result<Vec<u8>, AppError> {
        let (file_data, block_ids) = Self::download_file_stream(repos, repo_id, path).await?;

        let mut file_content = Vec::with_capacity(file_data.size as usize);
        for block_id in &block_ids {
            let block_data = block_store
                .read_block(block_id)
                .await
                .map_err(|e| AppError::internal(e.to_string()))?;
            // If decryption key is provided, decrypt the block.
            let block_data = if let Some((key, iv)) = dec_key {
                decrypt_block(&block_data, key, iv)
                    .map_err(|e| AppError::internal(e.to_string()))?
            } else {
                block_data
            };
            file_content.extend_from_slice(&block_data);
        }

        Ok(file_content)
    }

    /// Resolve a file's block IDs without reading their content.
    ///
    /// Returns `(FsFileData, Vec<block_id>)` so the caller can stream
    /// blocks individually without loading the entire file into memory.
    pub async fn resolve_blocks(
        repos: &Repositories,
        repo_id: &str,
        path: &str,
    ) -> Result<(FsFileData, Vec<String>), AppError> {
        Self::download_file_stream(repos, repo_id, path).await
    }

    pub async fn download_file_stream(
        repos: &Repositories,
        repo_id: &str,
        path: &str,
    ) -> Result<(FsFileData, Vec<String>), AppError> {
        // Resolve the path to a file fs_id by walking the FS tree from the
        // repo's head commit.
        let repo_model = repos
            .repo
            .find_by_id(repo_id)
            .await?
            .ok_or_else(|| AppError::NotFound("repo not found".into()))?;
        let head_commit_id = repo_model
            .head_commit_id
            .ok_or_else(|| AppError::NotFound("repo has no commits".into()))?;
        let head_commit = repos
            .commit
            .find_by_repo_and_commit_id(repo_id, &head_commit_id)
            .await?
            .ok_or_else(|| AppError::NotFound("head commit not found".into()))?;

        let fs_id =
            crate::fs::core::resolve_fs_id(repos, repo_id, &head_commit.root_id, path).await?;

        let file_data =
            crate::fs::core::file_ops::FileOps::read_file_fs_object(repos, repo_id, &fs_id).await?;

        Ok((file_data.clone(), file_data.block_ids))
    }
}

/// Build a streaming body that reads and yields blocks one at a time.
///
/// `block_ids` — list of block SHA-1 hashes to stream.
/// `block_store` — content-addressed block storage backend.
/// `enc_key` — optional decryption key (None = plaintext blocks).
pub fn stream_blocks(
    block_ids: Vec<String>,
    block_store: DynBlockStorage,
    enc_key: Option<(Vec<u8>, Vec<u8>)>,
) -> impl Stream<Item = Result<bytes::Bytes, std::io::Error>> + 'static {
    futures::stream::iter(block_ids.into_iter().map(move |block_id| {
        let store = block_store.clone();
        let key = enc_key.clone();
        async move {
            let data = store
                .read_block(&block_id)
                .await
                .map_err(|e| std::io::Error::other(e.to_string()))?;
            let data = match &key {
                Some((k, iv)) => {
                    decrypt_block(&data, k, iv).map_err(|e| std::io::Error::other(e.to_string()))?
                }
                None => data,
            };
            Ok(bytes::Bytes::from(data))
        }
    }))
    .buffered(4)
}
