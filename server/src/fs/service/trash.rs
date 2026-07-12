use std::sync::Arc;

use sea_orm::DatabaseConnection;

use crate::activity_log;
use crate::error::AppError;
use crate::repo::trash::TrashService;
use crate::repository::Repositories;

pub struct FsTrashService {
    repos: Arc<Repositories>,
    db: Arc<DatabaseConnection>,
}

impl FsTrashService {
    pub fn new(repos: Arc<Repositories>, db: Arc<DatabaseConnection>) -> Self {
        Self { repos, db }
    }

    fn db(&self) -> &DatabaseConnection {
        self.db.as_ref()
    }

    /// Page-based trash listing.
    pub async fn list_trash2(
        &self,
        repo_id: &str,
        page: u32,
        per_page: u32,
    ) -> Result<serde_json::Value, AppError> {
        let result = TrashService::list_trash2(self.db(), repo_id, page, per_page).await?;
        Ok(serde_json::to_value(result)?)
    }

    /// Cursor-based trash listing.
    pub async fn list_trash_cursor(
        &self,
        repo_id: &str,
        cursor: Option<i64>,
        limit: u32,
    ) -> Result<serde_json::Value, AppError> {
        let result = TrashService::list_trash_cursor(self.db(), repo_id, cursor, limit).await?;
        Ok(serde_json::to_value(result)?)
    }

    /// Search trash items.
    pub async fn search_trash(
        &self,
        repo_id: &str,
        q: &str,
        page: u32,
        per_page: u32,
        op_users: Option<&str>,
        time_from: Option<i64>,
        time_to: Option<i64>,
        suffixes: Option<&str>,
    ) -> Result<serde_json::Value, AppError> {
        let result = TrashService::search_trash(
            self.db(),
            repo_id,
            q,
            page,
            per_page,
            op_users,
            time_from,
            time_to,
            suffixes,
        )
        .await?;
        Ok(serde_json::to_value(result)?)
    }

    /// Restore multiple trash items by commit_id -> paths mapping.
    pub async fn revert_trash(
        &self,
        repo_id: &str,
        email: &str,
        user_id: i32,
        body: std::collections::HashMap<String, Vec<String>>,
    ) -> Result<serde_json::Value, AppError> {
        let result = TrashService::restore_trash_items(
            self.db(),
            &self.repos,
            repo_id,
            email,
            user_id,
            body,
        )
        .await?;
        Ok(serde_json::to_value(result)?)
    }

    /// Restore specific dirents (old API).
    pub async fn revert_dirents(
        &self,
        repo_id: &str,
        email: &str,
        user_id: i32,
        commit_id: &str,
        paths: Vec<String>,
    ) -> Result<serde_json::Value, AppError> {
        let result = TrashService::restore_dirents(
            self.db(),
            &self.repos,
            repo_id,
            email,
            user_id,
            commit_id,
            paths,
        )
        .await?;
        Ok(serde_json::to_value(result)?)
    }

    /// Clean trash, optionally keeping items newer than `keep_days`.
    pub async fn clean_trash(
        &self,
        repo_id: &str,
        user_id: i32,
        keep_days: Option<i64>,
    ) -> Result<(), AppError> {
        TrashService::clean_trash(self.db(), repo_id, keep_days).await?;

        activity_log::log_activity(
            self.db(),
            repo_id,
            "clean-up-trash",
            "repo",
            "/",
            user_id,
            None,
            None,
            None,
            None,
            keep_days,
        )
        .await;

        Ok(())
    }

    /// List deleted repos for the user.
    pub async fn list_deleted_repos(
        &self,
        user_id: i32,
        email: &str,
    ) -> Result<Vec<serde_json::Value>, AppError> {
        let repos = TrashService::list_deleted_repos(&self.repos, user_id).await?;

        let owner_name = self
            .repos
            .user
            .find_by_id(user_id)
            .await?
            .map(|u| u.nickname())
            .unwrap_or_else(|| email.split('@').next().unwrap_or("").to_string());

        let items: Vec<serde_json::Value> = repos
            .iter()
            .map(|r| {
                serde_json::json!({
                    "repo_id": r.repo_id,
                    "repo_name": r.repo_name,
                    "owner_email": email,
                    "owner_name": &owner_name,
                    "owner_contact_email": email,
                    "head_commit_id": r.head_id,
                    "size": r.size,
                    "del_time": chrono::DateTime::from_timestamp(r.del_time, 0)
                        .map(|d| d.to_rfc3339())
                        .unwrap_or_default(),
                    "org_id": -1,
                    "encrypted": false,
                })
            })
            .collect();

        Ok(items)
    }

    /// Restore a deleted repo.
    pub async fn restore_deleted_repo(&self, repo_id: &str, user_id: i32) -> Result<(), AppError> {
        TrashService::restore_deleted_repo(self.db(), &self.repos, repo_id, user_id).await?;
        Ok(())
    }
}
