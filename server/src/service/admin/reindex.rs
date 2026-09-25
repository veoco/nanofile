use std::sync::Arc;

use crate::indexer::{DocMeta, DocStatus, TextIndexer};
use crate::repository::Repositories;
use base::error::AppError;

/// Index/reindex administration: who may touch a library's index, and the
/// manual text-injection endpoint.
///
/// The pipeline itself lives in [`crate::service::index`]; this type is the
/// access-control layer in front of it.
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

    /// Verify that the user is the repo owner or a server admin.
    /// Stricter than `check_repo_access` — used for heavy operations
    /// (full-text reindex) that must not be triggerable by read-only members.
    pub async fn check_repo_admin(
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
            let user = self
                .repos
                .user
                .find_by_id(user_id)
                .await?
                .ok_or(AppError::Forbidden)?;
            if !user.is_admin {
                return Err(AppError::Forbidden);
            }
        }
        Ok(repo_model)
    }

    /// Index a single file with custom extracted text.
    ///
    /// The document is pinned at [`DocMeta::MANUAL_VERSION`], which is what
    /// stops the automatic backfill from overwriting text a client supplied for
    /// a format the server cannot parse itself.
    pub async fn index_file_text(
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

        let meta = DocMeta {
            fs_id: "manual".to_string(),
            extractor_version: DocMeta::MANUAL_VERSION,
            status: DocStatus::Indexed,
            attempted_at: chrono::Utc::now().timestamp(),
        };
        indexer
            .index_file_with_meta_async(repo_id, &fullpath, filename, text, meta)
            .await
            .map_err(|e| AppError::Internal(format!("index failed: {e}")))
    }
}
