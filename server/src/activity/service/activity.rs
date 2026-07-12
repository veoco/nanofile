use chrono::DateTime;
use sea_orm::DatabaseConnection;
use std::collections::HashMap;

use crate::repository::Repositories;
use base::error::AppError;

/// Service for activity-related operations.
pub struct ActivityService;

impl ActivityService {
    /// Returns paginated file activity events visible to the given user.
    ///
    /// - Without `op_user`: shows activities from all repos the user has access to
    ///   (via repo_membership), from ALL users.
    /// - With `op_user=<email>`: shows only that user's activities (Web "My Activities").
    /// - With `repo_id=<id>`: shows only activities in that specific repo.
    ///
    /// Response format matches seahub (api2/endpoints/activities.py).
    pub async fn get_activities(
        _db: &DatabaseConnection,
        repos: &Repositories,
        user_id: i32,
        page: u32,
        per_page: u32,
        repo_id: Option<&str>,
        op_user: Option<&str>,
        avatar_size: u32,
    ) -> Result<serde_json::Value, AppError> {
        let page = page.max(1);
        let per_page = per_page.clamp(1, 100);
        let offset = ((page - 1) * per_page) as u64;

        // Get all repo_ids the current user has access to
        let accessible_repo_ids: Vec<String> = repos
            .member
            .find_by_user_id(user_id)
            .await?
            .into_iter()
            .map(|m| m.repo_id)
            .collect();

        // Build base query: activities in repos the user can access
        let mut need_filter_user_id: Option<i32> = None;
        let mut need_filter_repo_id: Option<String> = None;

        // If op_user is specified (Web "My Activities" mode), filter by that user
        if let Some(email) = op_user {
            match repos.user.find_by_email(email).await? {
                Some(u) => {
                    need_filter_user_id = Some(u.id);
                }
                None => {
                    // User not found — return empty result
                    return Ok(serde_json::json!({
                        "events": [],
                        "page": page,
                        "per_page": per_page,
                        "total_count": 0,
                    }));
                }
            }
        }

        // If repo_id is specified, verify it's accessible then filter
        if let Some(rid) = repo_id {
            if !accessible_repo_ids.contains(&rid.to_string()) {
                return Ok(serde_json::json!({
                    "events": [],
                    "page": page,
                    "per_page": per_page,
                    "total_count": 0,
                }));
            }
            need_filter_repo_id = Some(rid.to_string());
        }

        // When not filtering by a specific op_user, include the current
        // user's own activities directly (matching the original seahub
        // UserActivity fan-out semantics — users always see their own actions).
        let direct_user_id = if need_filter_user_id.is_none() {
            Some(user_id)
        } else {
            None
        };

        // Get total count for pagination metadata
        let total_count = repos
            .activity
            .count_by_repo_ids_filtered(
                accessible_repo_ids.clone(),
                need_filter_user_id,
                need_filter_repo_id.as_deref(),
                direct_user_id,
            )
            .await?;

        // Fetch paginated activities
        let events = repos
            .activity
            .find_by_repo_ids_filtered(
                accessible_repo_ids,
                need_filter_user_id,
                need_filter_repo_id.as_deref(),
                offset,
                per_page as u64,
                direct_user_id,
            )
            .await?;

        // Batch-load user models for nickname, email, etc.
        let mut user_cache: HashMap<i32, infra::entity::user::Model> = HashMap::new();
        let mut user_ids: Vec<i32> = events.iter().map(|e| e.user_id).collect();
        user_ids.sort();
        user_ids.dedup();
        for uid in &user_ids {
            if let Ok(Some(u)) = repos.user.find_by_id(*uid).await {
                user_cache.insert(*uid, u);
            }
        }

        // Batch-load repo names
        let mut repo_names: HashMap<String, String> = HashMap::new();
        for e in &events {
            if let Ok(Some(r)) = repos.repo.find_by_id(&e.repo_id).await {
                repo_names.entry(e.repo_id.clone()).or_insert(r.name);
            }
        }

        // Build event list
        let mut event_list: Vec<serde_json::Value> = Vec::with_capacity(events.len());
        for e in &events {
            let u = user_cache.get(&e.user_id);
            let email = u.map(|u| u.email.as_str()).unwrap_or("");
            let repo_name = repo_names.get(&e.repo_id).map(|s| s.as_str()).unwrap_or("");
            let name = e
                .path
                .rsplit_once('/')
                .map(|(_, n)| n.to_string())
                .unwrap_or_default();
            let time = DateTime::from_timestamp(e.created_at, 0)
                .map(|dt| dt.to_rfc3339())
                .unwrap_or_default();

            // Parse the stored detail JSON (seafevents format).
            // Single ops store a dict, batch ops store an array.
            let (details, count) = match serde_json::from_str::<serde_json::Value>(&e.detail) {
                Ok(serde_json::Value::Array(arr)) => (arr.clone(), arr.len()),
                Ok(serde_json::Value::Object(_)) => {
                    (vec![serde_json::from_str(&e.detail).unwrap_or_default()], 1)
                }
                _ => (vec![], 0),
            };

            // Prefer repo_name from detail (event-time name) over current DB name.
            let event_repo_name = details
                .first()
                .and_then(|d| d.get("repo_name"))
                .and_then(|v| v.as_str())
                .unwrap_or(repo_name);

            // author_name: use full nickname() fallback chain (display_name -> name -> email local part)
            let author_name = u
                .map(|u| u.nickname())
                .unwrap_or_else(|| email.split('@').next().unwrap_or("").to_string());

            // login_id: match seahub fallback behavior — use email local part
            let login_id = email.split('@').next().unwrap_or("").to_string();

            let mut d = serde_json::json!({
                "op_type": e.op_type,
                "repo_id": e.repo_id,
                "repo_name": event_repo_name,
                "obj_type": e.obj_type,
                "commit_id": e.commit_id,
                "path": e.path,
                "name": name,
                "author_email": email,
                "author_name": author_name,
                "author_contact_email": email,
                "login_id": login_id,
                "avatar_url": crate::user::service::primary_avatar_url(email, avatar_size),
                "time": time,
                "details": details,
                "count": count,
            });

            // Include old_path for rename/move operations
            if let Some(ref old_path) = e.old_path {
                d["old_path"] = serde_json::json!(old_path);
            }

            // old_name for rename events (seahub API compatibility)
            if e.op_type == "rename"
                && (e.obj_type == "file" || e.obj_type == "dir")
                && let Some(ref old_path) = e.old_path
            {
                d["old_name"] =
                    serde_json::json!(old_path.rsplit_once('/').map(|(_, n)| n).unwrap_or(""));
            }

            // old_repo_name for repo rename events (from detail JSON)
            if !details.is_empty()
                && let Some(orn) = details[0].get("old_repo_name")
            {
                d["old_repo_name"] = orn.clone();
            }

            // days for clean-up-trash events (from detail JSON)
            if e.op_type == "clean-up-trash"
                && !details.is_empty()
                && let Some(days_val) = details[0].get("days")
            {
                d["days"] = days_val.clone();
            }

            // old_path for publish events
            if e.op_type == "publish"
                && let Some(ref old_path) = e.old_path
            {
                d["old_path"] = serde_json::json!(old_path);
            }

            event_list.push(d);
        }

        Ok(serde_json::json!({
            "events": event_list,
            "page": page,
            "per_page": per_page,
            "total_count": total_count,
        }))
    }
}
