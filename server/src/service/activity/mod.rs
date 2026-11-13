//! Activity and notification services.

use std::collections::HashMap;

use crate::repository::Repositories;
use base::error::AppError;
use infra::common::util::timestamp_rfc3339;
use infra::entity::{activity, activity_item};

/// Largest page size the activity list accepts. The endpoint's own Web UI asks for 25; the ceiling
/// exists so one authenticated request cannot ask the database for the entire activity table.
pub const MAX_ACTIVITY_PER_PAGE: u32 = 100;

/// Detail rows returned per activity. The summary's `item_count` remains exact
/// for larger operations, matching seafevents' 200-item detail cap.
pub const ACTIVITY_DETAIL_LIMIT: usize = 200;

/// Hydrated view of one activity summary: the detail objects and count that the
/// seahub-compatible API and the Web UI render. New rows read `activity_items`;
/// rows written before the split (item_count = 0) fall back to `detail` JSON.
pub struct ActivityDisplay {
    pub detail_value: serde_json::Value,
    pub details: serde_json::Value,
    pub count: usize,
    pub repo_name: String,
    pub old_repo_name: Option<String>,
    pub merged: bool,
    pub merge_kept: bool,
}

fn detail_repo_name(detail: &serde_json::Value) -> String {
    match detail {
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
    }
}

fn detail_old_repo_name(detail: &serde_json::Value) -> Option<String> {
    let value = match detail {
        serde_json::Value::Object(obj) => obj.get("old_repo_name"),
        serde_json::Value::Array(arr) => arr.first().and_then(|d| d.get("old_repo_name")),
        _ => None,
    };
    value.and_then(|v| v.as_str()).map(str::to_string)
}

fn detail_flag(detail: &serde_json::Value, key: &str) -> bool {
    match detail {
        serde_json::Value::Object(obj) => obj.get(key).and_then(|v| v.as_bool()).unwrap_or(false),
        serde_json::Value::Array(arr) => arr
            .iter()
            .any(|d| d.get(key).and_then(|v| v.as_bool()).unwrap_or(false)),
        _ => false,
    }
}

/// Load `activity_items` for a page and shape each summary for the API/UI.
pub async fn load_activity_displays(
    repos: &Repositories,
    activities: &[activity::Model],
    detail_limit: usize,
) -> Result<HashMap<i32, ActivityDisplay>, AppError> {
    // Rows written before the split carry no item rows; skip their lookups.
    let ids: Vec<i32> = activities
        .iter()
        .filter(|a| a.item_count > 0)
        .map(|a| a.id)
        .collect();
    let items = repos
        .activity
        .find_items_by_activity_ids(&ids, detail_limit as u64)
        .await?;
    let mut by_activity: HashMap<i32, Vec<activity_item::Model>> = HashMap::new();
    for item in items {
        by_activity.entry(item.activity_id).or_default().push(item);
    }

    let mut out = HashMap::with_capacity(activities.len());
    for a in activities {
        let detail_value: serde_json::Value = serde_json::from_str(&a.detail).unwrap_or_default();
        let repo_name = detail_repo_name(&detail_value);
        let old_repo_name = detail_old_repo_name(&detail_value);
        let items = by_activity.get(&a.id);

        let (details, count, merged, merge_kept) = if a.item_count > 0 || items.is_some() {
            let count = if a.item_count > 0 {
                a.item_count as usize
            } else {
                items.map_or(0, Vec::len)
            };
            let details: Vec<serde_json::Value> = items
                .into_iter()
                .flatten()
                .take(detail_limit)
                .map(|item| {
                    let mut entry = serde_json::json!({ "path": item.path });
                    if let Some(old_path) = &item.old_path {
                        entry["old_path"] = serde_json::json!(old_path);
                    }
                    if let Some(size) = item.size {
                        entry["size"] = serde_json::json!(size);
                    }
                    if let Some(obj_id) = &item.obj_id {
                        entry["obj_id"] = serde_json::json!(obj_id);
                    }
                    if !repo_name.is_empty() {
                        entry["repo_name"] = serde_json::json!(repo_name);
                    }
                    if item.merge {
                        entry["merge"] = serde_json::json!(true);
                    }
                    if item.merge_kept {
                        entry["merge_kept"] = serde_json::json!(true);
                    }
                    entry
                })
                .collect();
            let merged = detail_flag(&detail_value, "merge")
                || items.into_iter().flatten().any(|item| item.merge);
            let merge_kept = detail_flag(&detail_value, "merge_kept")
                || items.into_iter().flatten().any(|item| item.merge_kept);
            (serde_json::Value::Array(details), count, merged, merge_kept)
        } else {
            match &detail_value {
                serde_json::Value::Array(arr) => (
                    detail_value.clone(),
                    arr.len(),
                    detail_flag(&detail_value, "merge"),
                    detail_flag(&detail_value, "merge_kept"),
                ),
                serde_json::Value::Object(_) => (
                    serde_json::json!([detail_value]),
                    1,
                    detail_flag(&detail_value, "merge"),
                    detail_flag(&detail_value, "merge_kept"),
                ),
                _ => (serde_json::json!([]), 0, false, false),
            }
        };

        out.insert(
            a.id,
            ActivityDisplay {
                detail_value,
                details,
                count,
                repo_name,
                old_repo_name,
                merged,
                merge_kept,
            },
        );
    }
    Ok(out)
}

pub const MAX_ACTIVITY_PAGE: u32 = 10_000;

pub struct ActivityService;

impl ActivityService {
    /// `base_url` is the site origin (`scheme://host[:port]`) used to build the absolute `avatar_url`
    /// (matching seahub's `api_avatar_url`).
    pub async fn get_activities(
        repos: &Repositories,
        base_url: &str,
        user_id: i32,
        page: u32,
        per_page: u32,
        repo_id: Option<&str>,
        op_user: Option<&str>,
        repo_scope: &crate::domain::permission::RepoScope,
    ) -> Result<serde_json::Value, AppError> {
        // Clamp defensively: `per_page` becomes the SQL LIMIT, so an unbounded
        // value pulls the whole filtered table into memory.
        let page = page.clamp(1, MAX_ACTIVITY_PAGE);
        let per_page = per_page.clamp(1, MAX_ACTIVITY_PER_PAGE);
        let offset = u64::from(page - 1) * u64::from(per_page);

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
        // A key bound to specific libraries must not learn about the others
        // through the activity feed.
        repo_ids.retain(|id| repo_scope.allows(id));

        // An email that matches no user yields an empty page.
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

        // Batch-fetch the distinct users referenced by this page (avoid N+1).
        let ids: Vec<i32> = activities.iter().map(|a| a.user_id).collect();
        let users_by_id: std::collections::HashMap<i32, (String, String)> = repos
            .user
            .find_by_ids(&ids)
            .await?
            .into_iter()
            .map(|u| (u.id, (u.nickname(), u.email)))
            .collect();

        let displays = load_activity_displays(repos, &activities, ACTIVITY_DETAIL_LIMIT).await?;

        let mut items: Vec<serde_json::Value> = Vec::new();
        for a in &activities {
            let Some(display) = displays.get(&a.id) else {
                continue;
            };

            let (user_name, user_email) = users_by_id.get(&a.user_id).cloned().unwrap_or_default();

            // Absolute avatar URL (seahub's `api_avatar_url`; the Android client loads this field
            // directly with Glide). When the author no longer exists (empty email), a placeholder
            // keeps the URL well-formed; the avatar route then serves the default.
            let avatar_url = format!(
                "{}{}",
                base_url,
                crate::service::user::primary_avatar_url(
                    if user_email.is_empty() {
                        "deleted"
                    } else {
                        &user_email
                    },
                    256
                )
            );

            let details = display.details.clone();
            let count = display.count;
            let old_repo_name = display.old_repo_name.clone();
            // repo_name from detail, set at event time by log_activity, so it reflects the name when
            // the activity occurred.
            let repo_name = display.repo_name.clone();
            let detail_value = &display.detail_value;

            // name is the basename of path. Official seahub: repo events always have path="/", so
            // name="". Exception: nanofile's own repo-rename events (no seahub equivalent) keep the
            // new repo name so the Android client renders "old_name => name" correctly.
            let name = if a.obj_type == "repo" {
                if a.op_type == "rename" {
                    repo_name.clone()
                } else {
                    String::new()
                }
            } else {
                a.path
                    .rsplit_once('/')
                    .map(|(_, n)| n.to_string())
                    .unwrap_or_default()
            };

            // old_name = basename of old_path. Rename events recorded by older builds carry a null
            // old_path; fall back to old_repo_name so the Android client (which renders rename as
            // "old_name => name") never receives a null old_name.
            let old_name = a
                .old_path
                .as_deref()
                .and_then(|p| p.rsplit_once('/').map(|(_, n)| n.to_string()))
                .or_else(|| {
                    if a.obj_type == "repo" && a.op_type == "rename" {
                        old_repo_name.clone()
                    } else {
                        None
                    }
                });

            // Official seahub conditionally emits old_path (move/rename), old_name (rename) and
            // old_repo_name (rename+repo). We emit them on every event because seadroid renders
            // renames as "old_name => name" and NPEs on a missing old_name (an official client-side
            // bug). Aligning would break the Android client; revisit in a future major version.
            let mut ev = serde_json::json!({
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
                // nanofile has no login_id/profile concept; official seahub returns "" when the
                // author has none.
                "login_id": "",
                "avatar_url": avatar_url,
                "user_name": user_name,
                "user_email": user_email,
                "details": details,
                "count": count,
                "old_repo_name": old_repo_name,
                "time": timestamp_rfc3339(a.created_at),
            });

            // Official seahub emits a top-level days field only for clean-up-trash events.
            if a.op_type == "clean-up-trash"
                && let Some(days) = detail_value.get("days").and_then(|v| v.as_i64())
            {
                ev["days"] = serde_json::json!(days);
            }

            items.push(ev);
        }

        Ok(serde_json::json!({"events": items, "total_count": total}))
    }
}

/// Returns the count of unseen notifications.
/// Always returns 0 as nanofile doesn't have a notification system.
pub fn get_unseen_messages() -> i32 {
    0
}
