use sea_orm::DatabaseConnection;

use crate::crypto::random_key::decrypt_block;
use crate::repository::Repositories;
use crate::serialization::fs_json::FsFileData;
use crate::storage::DynBlockStorage;

pub struct Downloader;

impl Downloader {
    pub async fn download_file(
        repos: &Repositories,
        db: &DatabaseConnection,
        repo_id: &str,
        path: &str,
        block_store: &DynBlockStorage,
        // Optional decryption key (key, iv) — when set, blocks are decrypted
        // after reading. Used for encrypted repos during web download.
        dec_key: Option<(&[u8], &[u8])>,
    ) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
        let (file_data, block_ids) = Self::download_file_stream(repos, db, repo_id, path).await?;

        let mut file_content = Vec::with_capacity(file_data.size as usize);
        for block_id in &block_ids {
            let block_data = block_store.read_block(block_id).await?;
            // If decryption key is provided, decrypt the block.
            let block_data = if let Some((key, iv)) = dec_key {
                decrypt_block(&block_data, key, iv)?
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
        db: &DatabaseConnection,
        repo_id: &str,
        path: &str,
    ) -> Result<(FsFileData, Vec<String>), Box<dyn std::error::Error>> {
        Self::download_file_stream(repos, db, repo_id, path).await
    }

    pub async fn download_file_stream(
        repos: &Repositories,
        db: &DatabaseConnection,
        repo_id: &str,
        path: &str,
    ) -> Result<(FsFileData, Vec<String>), Box<dyn std::error::Error>> {
        // Resolve the path to a file fs_id by walking the FS tree from the
        // repo's head commit.
        let repo_model = repos
            .repo
            .find_by_id(repo_id)
            .await?
            .ok_or_else(|| "repo not found".to_string())?;
        let head_commit_id = repo_model
            .head_commit_id
            .ok_or_else(|| "repo has no commits".to_string())?;
        let head_commit = repos
            .commit
            .find_by_repo_and_commit_id(repo_id, &head_commit_id)
            .await?
            .ok_or_else(|| "head commit not found".to_string())?;

        let fs_id = crate::repo::resolve_fs_id(db, repo_id, &head_commit.root_id, path).await?;

        let file_data =
            crate::repo::file_ops::FileOps::read_file_fs_object(db, repo_id, &fs_id).await?;

        Ok((file_data.clone(), file_data.block_ids))
    }
}
