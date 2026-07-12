//! Activity and notification services.

use chrono::DateTime;
use sea_orm::DatabaseConnection;
use std::collections::HashMap;

use crate::repository::Repositories;
use base::error::AppError;

/// Service for activity-related operations.
pub struct ActivityService;

impl ActivityService {
    /// Get activity logs with optional repo_id and user_id filtering.
    pub async fn get_activities(
        _db: &DatabaseConnection,
        repos: &Repositories,
        user_id: i32,
        page: u32,
        per_page: u32,
        repo_id: Option<&str>,
        _op_user: Option<&str>,
        _avatar_size: u32,
    ) -> Result<serde_json::Value, AppError> {
        let offset = ((page.saturating_sub(1)) * per_page) as u64;

        // Fetch activities visible to this user.
        // Build a list of accessible repo IDs (owned + shared), then query
        // with directuser_id so the user's own actions are always included.
        let mut repo_ids: Vec<String> = repos
            .repo
            .find_by_owner_id(user_id)
            .await?
            .into_iter()
            .map(|r| r.id)
            .collect();
        for m in &repos.member.find_by_user_id(user_id).await? {
            if !repo_ids.contains(&m.repo_id) {
                repo_ids.push(m.repo_id.clone());
            }
        }

        let activities = repos
            .activity
            .find_by_repo_ids_filtered(
                repo_ids.clone(),
                None,
                repo_id,
                offset,
                per_page as u64,
                Some(user_id),
            )
            .await?;

        // Also fetch total count (separate query to avoid counting paginated results).
        let total = repos
            .activity
            .count_by_repo_ids_filtered(
                repo_ids,
                None,
                repo_id,
                Some(user_id),
            )
            .await?;

        let mut items: Vec<serde_json::Value> = Vec::new();
        for a in &activities {
            let detail: HashMap<String, serde_json::Value> =
                serde_json::from_str(&a.detail).unwrap_or_default();

            let user = repos.user.find_by_id(a.user_id).await?;
            let (user_name, user_email) = match user {
                Some(u) => (u.nickname(), u.email),
                None => (String::new(), String::new()),
            };

            items.push(serde_json::json!({
                "id": a.id,
                "op_type": a.op_type,
                "obj_type": a.obj_type,
                "path": a.path,
                "old_path": a.old_path,
                "commit_id": a.commit_id,
                "name": detail.get("name").or_else(|| detail.get("file_name")).and_then(|v| v.as_str()).unwrap_or(""),
                "author_email": user_email,
                "author_name": user_name,
                "user_name": user_name,
                "user_email": user_email,
                "detail": detail,
                "time": DateTime::from_timestamp(a.created_at, 0)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default(),
            }));
        }

        Ok(serde_json::json!({"events": items, "total_count": total}))
    }
}

/// Returns the count of unseen notifications.
/// Always returns 0 as nanofile doesn't have a notification system.
pub fn get_unseen_messages() -> i32 {
    0
}
