use std::sync::Arc;

use futures::StreamExt;

use super::collect_file_paths;
use crate::indexer::TextIndexer;
use crate::repository::Repositories;
use base::error::AppError;
use infra::common::EMPTY_SHA1;
use infra::storage::DynBlockStorage;

/// Service for index/reindex administration operations.
pub struct AdminService {
    repos: Arc<Repositories>,
}

impl AdminService {
    pub fn new(repos: Arc<Repositories>) -> Self {
        Self { repos }
    }

    /// Verify that the authenticated user can access a repository.
    pub async fn check_repo_access(
        &self,
        repo_id: &str,
        user_id: i32,
    ) -> Result<infra::entity::repo::Model, AppError> {
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
    ///
    /// Files are reindexed with bounded concurrency (whole-file reads +
    /// Tantivy writes are heavy); `on_progress` is called after each file with
    /// `(done_count, total)` so a background task can report progress.
    pub async fn reindex(
        &self,
        indexer: &TextIndexer,
        repo_id: &str,
        block_store: &DynBlockStorage,
        mut on_progress: impl FnMut(u64, u64) + Send + 'static,
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
        let total = file_paths.len() as u64;

        let results: Vec<Result<bool, AppError>> = futures::stream::iter(file_paths)
            .map(|fullpath| {
                let indexer = indexer.clone();
                let block_store = block_store.clone();
                let rid = repo_id.to_string();
                async move {
                    Ok(indexer
                        .reindex_file(&rid, &fullpath, &block_store)
                        .await
                        .unwrap_or(false))
                }
            })
            .buffer_unordered(8)
            .collect::<Vec<_>>()
            .await;

        let mut indexed = 0u64;
        let mut skipped = 0u64;
        let mut done = 0u64;
        for r in results {
            match r {
                Ok(true) => indexed += 1,
                _ => skipped += 1,
            }
            done += 1;
            on_progress(done, total);
        }

        Ok((indexed, skipped))
    }
}
