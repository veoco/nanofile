//! Activity and notification services.

use crate::repository::Repositories;
use base::error::AppError;
use chrono::DateTime;

/// Service for activity-related operations.
pub struct ActivityService;

impl ActivityService {
    /// Get activity logs with optional repo_id and user_id filtering.
    pub async fn get_activities(
        repos: &Repositories,
        user_id: i32,
        page: u32,
        per_page: u32,
        repo_id: Option<&str>,
        op_user: Option<&str>,
    ) -> Result<serde_json::Value, AppError> {
        let offset = ((page.saturating_sub(1)) * per_page) as u64;

        // Build list of accessible repo IDs (owned + shared).
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

        // Look up op_user by email if provided.
        // If the email doesn't match any user, return empty immediately.
        let op_user_id = match op_user {
            Some(email) => match repos.user.find_by_email(email).await? {
                Some(u) => Some(u.id),
                None => {
                    return Ok(serde_json::json!({"events": [], "total_count": 0}));
                }
            },
            None => None,
        };

        let activities = repos
            .activity
            .find_by_repo_ids_filtered(
                repo_ids.clone(),
                op_user_id,
                repo_id,
                offset,
                per_page as u64,
                Some(user_id),
            )
            .await?;

        // Also fetch total count (separate query to avoid counting paginated results).
        let total = repos
            .activity
            .count_by_repo_ids_filtered(repo_ids, op_user_id, repo_id, Some(user_id))
            .await?;

        let mut items: Vec<serde_json::Value> = Vec::new();
        for a in &activities {
            let detail_value: serde_json::Value = serde_json::from_str(&a.detail)
                .unwrap_or(serde_json::Value::Object(Default::default()));

            let user = repos.user.find_by_id(a.user_id).await?;
            let (user_name, user_email) = match user {
                Some(u) => (u.nickname(), u.email),
                None => (String::new(), String::new()),
            };

            // Build details array, count, and extract old_repo_name.
            // detail is an Object for single events and an Array for batch-aggregated events.
            let (details, count, old_repo_name) = match &detail_value {
                serde_json::Value::Array(arr) => {
                    let orn = arr
                        .first()
                        .and_then(|d| d.get("old_repo_name"))
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string());
                    (detail_value.clone(), arr.len(), orn)
                }
                serde_json::Value::Object(_) => {
                    let orn = detail_value
                        .get("old_repo_name")
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string());
                    (serde_json::json!([detail_value]), 1, orn)
                }
                _ => (serde_json::json!([]), 0, None),
            };

            // Extract repo_name from detail (set at event time by log_activity,
            // so it reflects the name when the activity occurred).
            let repo_name = match &detail_value {
                serde_json::Value::Object(obj) => obj
                    .get("repo_name")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string(),
                serde_json::Value::Array(arr) => arr
                    .first()
                    .and_then(|d| d.get("repo_name"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string(),
                _ => String::new(),
            };

            // name is the basename of path (or repo_name for repo-level events).
            let name = if a.obj_type == "repo" {
                repo_name.clone()
            } else {
                a.path
                    .rsplit_once('/')
                    .map(|(_, n)| n.to_string())
                    .unwrap_or_default()
            };

            // old_name = basename of old_path.
            let old_name = a
                .old_path
                .as_deref()
                .and_then(|p| p.rsplit_once('/').map(|(_, n)| n.to_string()));

            items.push(serde_json::json!({
                "id": a.id,
                "op_type": a.op_type,
                "obj_type": a.obj_type,
                "repo_id": a.repo_id,
                "repo_name": repo_name,
                "path": a.path,
                "old_path": a.old_path,
                "commit_id": a.commit_id,
                "name": name,
                "old_name": old_name,
                "author_email": user_email,
                "author_name": user_name,
                "author_contact_email": user_email,
                "user_name": user_name,
                "user_email": user_email,
                "details": details,
                "count": count,
                "old_repo_name": old_repo_name,
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
