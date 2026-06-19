use axum::{
    Json,
    extract::{Query, State},
};
use chrono::DateTime;
use sea_orm::{ColumnTrait, EntityTrait, PaginatorTrait, QueryFilter, QueryOrder, QuerySelect};
use serde::Deserialize;
use serde_json::Value;
use std::collections::HashMap;
use std::sync::Arc;

use crate::AppState;
use crate::auth::middleware::AuthUser;
use crate::entity::{activity, repo, repo_member, user};
use crate::error::AppError;

#[derive(Deserialize)]
pub struct ActivitiesQuery {
    pub page: Option<u32>,
    pub per_page: Option<u32>,
    pub repo_id: Option<String>,
    pub op_user: Option<String>,
    pub avatar_size: Option<u32>,
}

/// GET /api/v2.1/activities/
///
/// Returns paginated file activity events visible to the authenticated user.
///
/// - Without `op_user`: shows activities from all repos the user has access to
///   (via repo_membership), from ALL users. This matches what all three seafile
///   clients (Android, Desktop Qt, iOS) expect.
/// - With `op_user=<email>`: shows only that user's activities (Web "My Activities").
/// - With `repo_id=<id>`: shows only activities in that specific repo.
///
/// Response format matches seahub (api2/endpoints/activities.py).
pub async fn get_activities(
    auth: AuthUser,
    State(state): State<Arc<AppState>>,
    Query(query): Query<ActivitiesQuery>,
) -> Result<Json<serde_json::Value>, AppError> {
    let db = state.db.as_ref();
    let page = query.page.unwrap_or(1).max(1);
    let per_page = query.per_page.unwrap_or(25).clamp(1, 100);
    let offset = ((page - 1) * per_page) as u64;

    // Get all repo_ids the current user has access to
    let accessible_repo_ids: Vec<String> = repo_member::Entity::find()
        .filter(repo_member::Column::UserId.eq(auth.user_id))
        .all(db)
        .await?
        .into_iter()
        .map(|m| m.repo_id)
        .collect();

    // Build base query: activities in repos the user can access
    let mut base_filter = activity::Entity::find()
        .filter(activity::Column::RepoId.is_in(accessible_repo_ids.clone()));

    // If op_user is specified (Web "My Activities" mode), filter by that user
    if let Some(ref email) = query.op_user {
        match user::Entity::find()
            .filter(user::Column::Email.eq(email))
            .one(db)
            .await?
        {
            Some(u) => {
                base_filter = base_filter.filter(activity::Column::UserId.eq(u.id));
            }
            None => {
                // User not found — return empty result
                return Ok(Json(serde_json::json!({
                    "events": [],
                    "page": page,
                    "per_page": per_page,
                    "total_count": 0,
                })));
            }
        }
    }

    // If repo_id is specified, verify it's accessible then filter
    if let Some(ref rid) = query.repo_id {
        if !accessible_repo_ids.contains(rid) {
            return Ok(Json(serde_json::json!({
                "events": [],
                "page": page,
                "per_page": per_page,
                "total_count": 0,
            })));
        }
        base_filter = base_filter.filter(activity::Column::RepoId.eq(rid));
    }

    // Get total count for pagination metadata
    let total_count = base_filter.clone().count(db).await?;

    // Fetch paginated activities
    let events = base_filter
        .order_by_desc(activity::Column::CreatedAt)
        .offset(offset)
        .limit(per_page as u64)
        .all(db)
        .await?;

    // Batch-load user emails
    let mut user_emails: HashMap<i32, String> = HashMap::new();
    let mut user_ids: Vec<i32> = events.iter().map(|e| e.user_id).collect();
    user_ids.sort();
    user_ids.dedup();
    for uid in &user_ids {
        if let Ok(Some(u)) = user::Entity::find_by_id(*uid).one(db).await {
            user_emails.insert(*uid, u.email);
        }
    }

    // Batch-load repo names
    let mut repo_names: HashMap<String, String> = HashMap::new();
    for e in &events {
        if let Ok(Some(r)) = repo::Entity::find_by_id(&e.repo_id).one(db).await {
            repo_names.entry(e.repo_id.clone()).or_insert(r.name);
        }
    }

    // Build event list
    let mut event_list: Vec<serde_json::Value> = Vec::with_capacity(events.len());
    for e in &events {
        let email = user_emails
            .get(&e.user_id)
            .map(|s| s.as_str())
            .unwrap_or("");
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
        let (details, count) = match serde_json::from_str::<Value>(&e.detail) {
            Ok(Value::Array(arr)) => (arr.clone(), arr.len()),
            Ok(Value::Object(_)) => (vec![serde_json::from_str(&e.detail).unwrap_or_default()], 1),
            _ => (vec![], 0),
        };

        // Prefer repo_name from detail (event-time name) over current DB name.
        let event_repo_name = details
            .first()
            .and_then(|d| d.get("repo_name"))
            .and_then(|v| v.as_str())
            .unwrap_or(repo_name);

        let mut d = serde_json::json!({
            "op_type": e.op_type,
            "repo_id": e.repo_id,
            "repo_name": event_repo_name,
            "obj_type": e.obj_type,
            "commit_id": e.commit_id,
            "path": e.path,
            "name": name,
            "author_email": email,
            "author_name": email.split('@').next().unwrap_or(""),
            "author_contact_email": email,
            "login_id": "",
            "avatar_url": crate::api::avatar::primary_avatar_url(email, 32),
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

        event_list.push(d);
    }

    Ok(Json(serde_json::json!({
        "events": event_list,
        "page": page,
        "per_page": per_page,
        "total_count": total_count,
    })))
}
