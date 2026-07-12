use std::sync::Arc;

use sea_orm::DatabaseConnection;

use super::collect_file_paths;
use crate::common::EMPTY_SHA1;
use crate::error::AppError;
use crate::indexer::TextIndexer;
use crate::repository::Repositories;
use crate::storage::DynBlockStorage;

/// Service for index/reindex administration operations.
pub struct AdminService {
    db: Arc<DatabaseConnection>,
    repos: Arc<Repositories>,
}

impl AdminService {
    pub fn new(db: Arc<DatabaseConnection>, repos: Arc<Repositories>) -> Self {
        Self { db, repos }
    }

    /// Verify that the authenticated user can access a repository.
    pub async fn check_repo_access(
        &self,
        repo_id: &str,
        user_id: i32,
    ) -> Result<crate::entity::repo::Model, AppError> {
        let repo_model = self
            .repos
            .repo
            .find_by_id(repo_id)
            .await?
            .ok_or_else(|| AppError::NotFound("repo not found".into()))?;
        if repo_model.owner_id != user_id {
            let is_member = self
                .repos
                .member
                .find_by_repo_and_user(repo_id, user_id)
                .await?
                .is_some();
            if !is_member {
                return Err(AppError::Forbidden);
            }
        }
        Ok(repo_model)
    }

    /// Index a single file with custom extracted text.
    pub fn index_file_text(
        &self,
        indexer: &TextIndexer,
        repo_id: &str,
        path: &str,
        text: &str,
    ) -> Result<(), AppError> {
        let fullpath = if path.starts_with('/') {
            path.to_string()
        } else {
            format!("/{path}")
        };
        let filename = fullpath
            .rsplit_once('/')
            .map(|(_, name)| name)
            .unwrap_or(&fullpath);

        indexer
            .index_file(repo_id, &fullpath, filename, text)
            .map_err(|e| AppError::Internal(format!("index failed: {e}")))
    }

    /// Rebuild the full-text search index for all files in a repository.
    pub async fn reindex(
        &self,
        indexer: &TextIndexer,
        repo_id: &str,
        block_store: &DynBlockStorage,
    ) -> Result<(u64, u64), AppError> {
        let repo_model = self
            .repos
            .repo
            .find_by_id(repo_id)
            .await?
            .ok_or_else(|| AppError::NotFound("repo not found".into()))?;

        let head_commit_id = repo_model
            .head_commit_id
            .ok_or_else(|| AppError::NotFound("repo has no commits".into()))?;

        let head = self
            .repos
            .commit
            .find_by_id(&head_commit_id)
            .await?
            .ok_or_else(|| AppError::NotFound("head commit not found".into()))?;

        if head.root_id == EMPTY_SHA1 {
            return Ok((0, 0));
        }

        let file_paths = collect_file_paths(&self.repos, repo_id, &head.root_id).await?;

        let mut indexed = 0u64;
        let mut skipped = 0u64;

        for fullpath in &file_paths {
            match indexer
                .reindex_file(self.db.as_ref(), repo_id, fullpath, block_store)
                .await
            {
                Ok(true) => indexed += 1,
                Ok(false) => skipped += 1,
                Err(_) => skipped += 1,
            }
        }

        Ok((indexed, skipped))
    }
}
