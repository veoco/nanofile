use std::sync::Arc;

use crate::repository::Repositories;
use base::error::AppError;

pub struct MetadataService {
    repos: Arc<Repositories>,
}

impl MetadataService {
    pub fn new(repos: Arc<Repositories>) -> Self {
        Self { repos }
    }

    /// Get metadata config for a repo.
    pub async fn get_metadata_config(&self, repo_id: &str) -> Result<serde_json::Value, AppError> {
        let config = self.repos.metadata_config.find_by_repo_id(repo_id).await?;

        match config {
            Some(c) => Ok(serde_json::json!({"enabled": c.enabled})),
            None => Ok(serde_json::json!({"enabled": false})),
        }
    }

    /// Update metadata config for a repo.
    pub async fn update_metadata_config(
        &self,
        repo_id: &str,
        enabled: bool,
    ) -> Result<(), AppError> {
        self.repos.metadata_config.upsert(repo_id, enabled).await
    }

    /// Get file tags for a repo.
    pub async fn get_file_tags(&self, repo_id: &str) -> Result<Vec<String>, AppError> {
        let tags = self.repos.file_tag.find_by_repo_id(repo_id).await?;
        Ok(tags.into_iter().map(|t| t.tag_name).collect())
    }

    /// Update file tags for a specific file path.
    pub async fn update_file_tags(
        &self,
        repo_id: &str,
        file_path: &str,
        tags: Option<&[serde_json::Value]>,
    ) -> Result<(), AppError> {
        self.repos
            .file_tag
            .update_file_tags(repo_id, file_path, tags)
            .await
    }

    /// Get related users for a repo.
    pub async fn related_users(&self, repo_id: &str) -> Result<Vec<String>, AppError> {
        let members = self.repos.member.find_by_repo_id(repo_id).await?;
        Ok(members.into_iter().map(|m| m.user_id.to_string()).collect())
    }

    /// Get metadata records for a repo.
    pub async fn get_metadata_records(
        &self,
        repo_id: &str,
    ) -> Result<Vec<serde_json::Value>, AppError> {
        let records = self.repos.metadata_record.find_by_repo_id(repo_id).await?;

        let items: Vec<serde_json::Value> = records
            .into_iter()
            .map(|r| {
                serde_json::json!({
                    "file_path": r.file_path,
                    "key": r.record_key,
                    "value": r.record_value,
                })
            })
            .collect();

        Ok(items)
    }

    /// Create or update a metadata record.
    pub async fn update_metadata_record(
        &self,
        repo_id: &str,
        file_path: &str,
        key: &str,
        value: Option<&str>,
    ) -> Result<(), AppError> {
        self.repos
            .metadata_record
            .upsert(repo_id, file_path, key, value)
            .await
    }
}
