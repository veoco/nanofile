use sea_orm::{
    ActiveModelTrait, ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter, QueryOrder,
};

use crate::entity::{commit, repo};
use crate::repository::Repositories;
use base::common::FsDirData;

#[allow(dead_code)]
pub struct Versioning;

impl Versioning {
    pub async fn get_file_history(
        db: &DatabaseConnection,
        repos: &Repositories,
        repo_id: &str,
        path: &str,
        limit: u64,
    ) -> Result<Vec<crate::entity::commit::Model>, Box<dyn std::error::Error>> {
        let commits = commit::Entity::find()
            .filter(commit::Column::RepoId.eq(repo_id))
            .order_by_desc(commit::Column::Ctime)
            .all(db)
            .await?;

        let mut history = Vec::new();
        for c in commits {
            if history.len() >= limit as usize {
                break;
            }

            let root_data =
                crate::repo::file_ops::FileOps::read_dir_fs_object(repos, repo_id, &c.root_id)
                    .await?;

            if Self::path_exists_in_dir(db, repos, &root_data, path).await? {
                history.push(c);
            }
        }

        Ok(history)
    }

    async fn path_exists_in_dir(
        _db: &DatabaseConnection,
        repos: &Repositories,
        dir_data: &FsDirData,
        path: &str,
    ) -> Result<bool, Box<dyn std::error::Error>> {
        let parts: Vec<&str> = path.trim_start_matches('/').split('/').collect();
        if parts.is_empty() {
            return Ok(true);
        }

        let mut current_dir = dir_data.clone();
        for (i, part) in parts.iter().enumerate() {
            if i == parts.len() - 1 {
                return Ok(current_dir.dirents.iter().any(|e| e.name == *part));
            }
            let next_id = match current_dir
                .dirents
                .iter()
                .find(|e| e.name == *part && e.mode == crate::serialization::S_IFDIR)
                .map(|e| e.id.clone())
            {
                Some(id) => id,
                None => return Ok(false),
            };
            current_dir = crate::repo::read_fs_dir_data(repos, "", &next_id).await?;
        }
        Ok(false)
    }

    pub async fn revert_to_commit(
        db: &DatabaseConnection,
        repos: &Repositories,
        repo_id: &str,
        commit_id: &str,
        file_path: &str,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let target_commit = commit::Entity::find()
            .filter(commit::Column::RepoId.eq(repo_id))
            .filter(commit::Column::CommitId.eq(commit_id))
            .one(db)
            .await?
            .ok_or("commit not found")?;

        let parts: Vec<&str> = file_path.trim_start_matches('/').split('/').collect();
        let file_name = parts.last().ok_or("invalid file path")?;

        // Get the parent directory fs_id
        let parent_fs_id = if parts.len() > 1 {
            let parent_path = format!("/{}", parts[..parts.len() - 1].join("/"));
            crate::repo::resolve_fs_id(repos, repo_id, &target_commit.root_id, &parent_path).await?
        } else {
            target_commit.root_id.clone()
        };

        // Validate the file exists in the parent directory
        let parent_dir_data = crate::repo::read_fs_dir_data(repos, repo_id, &parent_fs_id).await?;
        let _target_fs_id = parent_dir_data
            .dirents
            .iter()
            .find(|e| e.name == *file_name)
            .map(|e| e.id.clone())
            .ok_or("file not found in target commit")?;

        let now = chrono::Utc::now().timestamp();

        let current_repo = repo::Entity::find_by_id(repo_id).one(db).await?;
        let current_head = current_repo.as_ref().and_then(|r| r.head_commit_id.clone());

        let new_commit_id = crate::crypto::fs_id::compute_commit_id(
            repo_id,
            &target_commit.root_id,
            current_head.as_deref(),
            now,
            &target_commit.creator_name,
            &format!("Reverted {} to version from {}", file_path, commit_id),
        );

        let commit_model = commit::ActiveModel {
            id: sea_orm::NotSet,
            repo_id: sea_orm::Set(repo_id.to_string()),
            commit_id: sea_orm::Set(new_commit_id.clone()),
            root_id: sea_orm::Set(target_commit.root_id),
            parent_id: sea_orm::Set(current_head),
            second_parent_id: sea_orm::NotSet,
            creator_name: sea_orm::Set(target_commit.creator_name.clone()),
            description: sea_orm::Set(format!(
                "Reverted {} to version from {}",
                file_path, commit_id
            )),
            ctime: sea_orm::Set(now),
            version: sea_orm::Set(1i8),
        };
        commit::Entity::insert(commit_model).exec(db).await?;

        if let Some(repo_model) = current_repo {
            let mut active: repo::ActiveModel = repo_model.into();
            active.head_commit_id = sea_orm::Set(Some(new_commit_id));
            active.updated_at = sea_orm::Set(now);
            active.update(db).await?;
        }

        Ok(())
    }
}
