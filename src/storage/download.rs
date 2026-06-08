use sea_orm::{ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter};

use crate::entity::{commit, repo};
use crate::serialization::fs_json::FsFileData;
use crate::storage::DynBlockStorage;

pub struct Downloader;

impl Downloader {
    pub async fn download_file(
        db: &DatabaseConnection,
        repo_id: &str,
        path: &str,
        block_store: &DynBlockStorage,
    ) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
        let (file_data, block_ids) = Self::download_file_stream(db, repo_id, path).await?;

        let mut file_content = Vec::with_capacity(file_data.size as usize);
        for block_id in &block_ids {
            let block_data = block_store.read_block(block_id).await?;
            file_content.extend_from_slice(&block_data);
        }

        Ok(file_content)
    }

    pub async fn download_file_stream(
        db: &DatabaseConnection,
        repo_id: &str,
        path: &str,
    ) -> Result<(FsFileData, Vec<String>), Box<dyn std::error::Error>> {
        // Resolve the path to a file fs_id by walking the FS tree from the
        // repo's head commit.
        let repo_model = repo::Entity::find_by_id(repo_id)
            .one(db)
            .await?
            .ok_or_else(|| "repo not found".to_string())?;
        let head_commit_id = repo_model
            .head_commit_id
            .ok_or_else(|| "repo has no commits".to_string())?;
        let head_commit = commit::Entity::find()
            .filter(commit::Column::CommitId.eq(&head_commit_id))
            .one(db)
            .await?
            .ok_or_else(|| "head commit not found".to_string())?;

        let fs_id =
            crate::storage::resolve_fs_id(db, repo_id, &head_commit.root_id, path, None).await?;

        let file_data =
            crate::storage::file_ops::FileOps::read_file_fs_object(db, repo_id, &fs_id).await?;

        Ok((file_data.clone(), file_data.block_ids))
    }
}
