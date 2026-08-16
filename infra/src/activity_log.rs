/// File activity event logger.
///
/// Provides a single entry point for recording user-initiated file operations
/// (create, rename, move, delete, edit) into the `activities` table.  Each
/// record captures the operation type, affected path, acting user, the repo's
/// current HEAD commit ID, and a JSON `detail` field containing size, obj_id,
/// path, and repo_name (matching the seafevents Activity.detail format).
///
/// # Batch aggregation
///
/// For `create` and `delete` operations, the logger attempts to aggregate
/// multiple operations within a 5-minute window into a single
/// `batch_create` / `batch_delete` record with an array of detail dicts.
/// This matches seafevents' `save_user_activity` + `_update_batch_activity`
/// logic.
///
/// # Best-effort semantics
///
/// All errors are logged via `tracing::warn!` but never propagated — activity
/// logging must not break the calling operation.
use sea_orm::{ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter, QueryOrder, Set};
use std::collections::HashMap;

use crate::entity::{activity, repo};

/// Maximum number of items in a batch-aggregated activity detail array.
const ACTIVITY_MAX_AGGREGATE_ITEMS: usize = 200;

/// Window (seconds) within which repeated edits of the same path collapse into
/// a single activity row (matching seafevents' `save_user_activities`).
const EDIT_DEDUP_WINDOW: i64 = 1800;

/// Log a file operation activity.
///
/// Parameters:
/// - `db`: database connection
/// - `repo_id`: the repository the operation occurred in
/// - `op_type`: one of `"create"`, `"delete"`, `"edit"`, `"rename"`, `"move"`
/// - `obj_type`: `"file"` or `"dir"` (or `"repo"` for repo-level events)
/// - `path`: the (new) file/directory path, e.g. `"/dir/file.txt"`
/// - `user_id`: numeric user ID from the `users` table
/// - `old_path`: previous path (for rename/move)
/// - `size`: file size in bytes (optional, for detail metadata)
/// - `obj_id`: fs_object ID (SHA1 hash, optional, for detail metadata)
///
/// The `op_type` and `obj_type` conventions match seafevents' constants:
/// seafevents uses `create`/`delete`/`edit`/`rename`/`move`/`recover`
/// as op_type and `file`/`dir` as obj_type.
///
/// For `create` / `delete` operations, the function attempts to aggregate
/// into a recent batch event within a 5-minute window (same repo, user,
/// and obj_type).  When aggregated, the record's `op_type` is promoted to
/// `batch_create` / `batch_delete` and the new detail item is appended to
/// the existing detail array.  `rename`, `move`, `edit`, and `recover`
/// operations are never aggregated, matching seafevents behavior.
#[allow(clippy::too_many_arguments)]
pub async fn log_activity(
    db: &DatabaseConnection,
    repo_id: &str,
    op_type: &str,
    obj_type: &str,
    path: &str,
    user_id: i32,
    old_path: Option<&str>,
    size: Option<i64>,
    obj_id: Option<&str>,
    old_repo_name: Option<&str>,
    days: Option<i64>,
) {
    let now = chrono::Utc::now().timestamp();

    // Best-effort: look up repo name and head commit ID.
    let (commit_id, repo_name) = match repo::Entity::find_by_id(repo_id).one(db).await {
        Ok(Some(r)) => {
            // Wiki repos are a separate product surface (知识库); their internal
            // file operations must not surface as ordinary library activities.
            if r.r#type == "wiki" {
                return;
            }
            (r.head_commit_id.unwrap_or_default(), r.name.clone())
        }
        _ => (String::new(), String::new()),
    };

    // Build detail dict (only non-None fields, matching seafevents pattern).
    let mut detail_map = serde_json::Map::new();
    if let Some(s) = size {
        detail_map.insert("size".to_string(), serde_json::json!(s));
    }
    if let Some(id) = obj_id {
        detail_map.insert("obj_id".to_string(), serde_json::json!(id));
    }
    detail_map.insert("path".to_string(), serde_json::json!(path));
    if !repo_name.is_empty() {
        detail_map.insert("repo_name".to_string(), serde_json::json!(repo_name));
    }
    if let Some(op) = old_path {
        detail_map.insert("old_path".to_string(), serde_json::json!(op));
    }
    if let Some(orn) = old_repo_name {
        detail_map.insert("old_repo_name".to_string(), serde_json::json!(orn));
    }
    if let Some(d) = days {
        detail_map.insert("days".to_string(), serde_json::json!(d));
    }

    // Only aggregate create/delete operations (matching seafevents'
    // BATCH_AGGREGATE_OP_TYPES = ('create', 'delete')).
    if op_type == "create" || op_type == "delete" {
        let batch_types = [op_type.to_string(), format!("batch_{}", op_type)];
        let cutoff = now - 300; // 5-minute window

        if let Ok(Some(recent)) = activity::Entity::find()
            .filter(activity::Column::RepoId.eq(repo_id))
            .filter(activity::Column::UserId.eq(user_id))
            .filter(activity::Column::ObjType.eq(obj_type))
            .filter(activity::Column::OpType.is_in(batch_types))
            .filter(activity::Column::CreatedAt.gt(cutoff))
            .order_by_desc(activity::Column::CreatedAt)
            .one(db)
            .await
        {
            // Found a recent aggregatable activity — try to append.
            if let Ok(current_detail) = serde_json::from_str::<serde_json::Value>(&recent.detail) {
                let mut detail_array: Vec<serde_json::Value> = match &current_detail {
                    serde_json::Value::Array(arr) => arr.clone(),
                    serde_json::Value::Object(_) => {
                        // Extract only the allowed detail keys (matching seafevents'
                        // _extract_detail_item behavior), so the first item in a batch
                        // array has the same shape as subsequently appended items.
                        let allowed_keys: [&str; 6] = [
                            "obj_id",
                            "size",
                            "old_path",
                            "repo_name",
                            "old_repo_name",
                            "path",
                        ];
                        let obj = current_detail.as_object().unwrap();
                        let filtered: serde_json::Value = allowed_keys
                            .iter()
                            .filter_map(|k| obj.get(*k).map(|v| ((*k).to_string(), v.clone())))
                            .collect::<serde_json::Map<_, _>>()
                            .into();
                        vec![filtered]
                    }
                    _ => Vec::new(),
                };

                if detail_array.len() < ACTIVITY_MAX_AGGREGATE_ITEMS {
                    detail_array.push(serde_json::Value::Object(detail_map));

                    let updated_detail =
                        serde_json::to_string(&detail_array).unwrap_or_else(|_| "[]".to_string());
                    let batch_op_type = format!("batch_{}", op_type);

                    let mut active: activity::ActiveModel = recent.into();
                    active.op_type = Set(batch_op_type);
                    active.detail = Set(updated_detail);
                    active.created_at = Set(now);
                    if let Err(e) = activity::Entity::update(active).exec(db).await {
                        tracing::warn!(
                            "Failed to update aggregated activity ({op_type} {path}): {e}"
                        );
                    }
                    return; // Successfully aggregated.
                }
            }
        }
    }

    // Repeated edits of the same path within the window only refresh the
    // existing record's timestamp (seafevents `save_user_activities`).
    if op_type == "edit" {
        let cutoff = now - EDIT_DEDUP_WINDOW;
        if let Ok(Some(recent)) = activity::Entity::find()
            .filter(activity::Column::RepoId.eq(repo_id))
            .filter(activity::Column::UserId.eq(user_id))
            .filter(activity::Column::Path.eq(path))
            .filter(activity::Column::OpType.eq("edit"))
            .filter(activity::Column::CreatedAt.gt(cutoff))
            .order_by_desc(activity::Column::CreatedAt)
            .one(db)
            .await
        {
            let mut active: activity::ActiveModel = recent.into();
            active.created_at = Set(now);
            if let Err(e) = activity::Entity::update(active).exec(db).await {
                tracing::warn!("Failed to update deduped edit activity ({path}): {e}");
            }
            return;
        }
    }

    // Insert a new activity record (single-operation or fallback).
    let detail_json = serde_json::to_string(&detail_map).unwrap_or_else(|_| "{}".to_string());

    if let Err(e) = activity::Entity::insert(activity::ActiveModel {
        id: sea_orm::NotSet,
        repo_id: Set(repo_id.to_string()),
        commit_id: Set(commit_id),
        op_type: Set(op_type.to_string()),
        obj_type: Set(obj_type.to_string()),
        path: Set(path.to_string()),
        old_path: Set(old_path.map(|s| s.to_string())),
        user_id: Set(user_id),
        created_at: Set(now),
        detail: Set(detail_json),
    })
    .exec(db)
    .await
    {
        tracing::warn!("Failed to log activity ({op_type} {path}): {e}");
    }
}

/// Lightweight activity entry for batched logging.
///
/// Kept in `infra` (no dependency on the server crate's `FsChange`) so the
/// sync commit path can log many items with one function call.
pub struct ActivityItem {
    pub op_type: &'static str,
    pub obj_type: &'static str,
    pub path: String,
    pub old_path: Option<String>,
    pub size: Option<i64>,
    pub obj_id: Option<String>,
}

/// Build the `detail` JSON object for a single item (mirrors `log_activity`).
fn build_detail_map(
    item: &ActivityItem,
    repo_name: &str,
) -> serde_json::Map<String, serde_json::Value> {
    let mut detail_map = serde_json::Map::new();
    if let Some(s) = item.size {
        detail_map.insert("size".to_string(), serde_json::json!(s));
    }
    if let Some(id) = &item.obj_id {
        detail_map.insert("obj_id".to_string(), serde_json::json!(id));
    }
    detail_map.insert("path".to_string(), serde_json::json!(item.path));
    if !repo_name.is_empty() {
        detail_map.insert("repo_name".to_string(), serde_json::json!(repo_name));
    }
    if let Some(op) = &item.old_path {
        detail_map.insert("old_path".to_string(), serde_json::json!(op));
    }
    detail_map
}

/// Construct an `activity::ActiveModel` row from an item.
fn to_active_model(
    repo_id: &str,
    commit_id: &str,
    repo_name: &str,
    user_id: i32,
    now: i64,
    item: &ActivityItem,
) -> activity::ActiveModel {
    let detail_json = serde_json::to_string(&build_detail_map(item, repo_name))
        .unwrap_or_else(|_| "{}".to_string());
    activity::ActiveModel {
        id: sea_orm::NotSet,
        repo_id: Set(repo_id.to_string()),
        commit_id: Set(commit_id.to_string()),
        op_type: Set(item.op_type.to_string()),
        obj_type: Set(item.obj_type.to_string()),
        path: Set(item.path.clone()),
        old_path: Set(item.old_path.clone()),
        user_id: Set(user_id),
        created_at: Set(now),
        detail: Set(detail_json),
    }
}

/// Parse a stored `detail` into the array of items it holds, reshaping a lone
/// object into a single-element array using only the allowed keys (matches the
/// single-item aggregation path).
fn parse_detail_array(detail: &str) -> Vec<serde_json::Value> {
    match serde_json::from_str::<serde_json::Value>(detail) {
        Ok(serde_json::Value::Array(arr)) => arr,
        Ok(serde_json::Value::Object(obj)) => {
            let allowed_keys: [&str; 6] = [
                "obj_id",
                "size",
                "old_path",
                "repo_name",
                "old_repo_name",
                "path",
            ];
            let filtered: serde_json::Value = allowed_keys
                .iter()
                .filter_map(|k| obj.get(*k).map(|v| ((*k).to_string(), v.clone())))
                .collect::<serde_json::Map<_, _>>()
                .into();
            vec![filtered]
        }
        _ => Vec::new(),
    }
}

/// Batch-log file activity on hot commit paths.
///
/// `commit_id` and `repo_name` are supplied by the caller (already fetched
/// once for the whole commit), so no per-item repo lookup is needed —
/// eliminating the N+1 query on large sync commits.
///
/// `create`/`delete` keep the 5-minute batch-aggregation semantics of
/// `log_activity`, grouped by `(op_type, obj_type)` so only one aggregation
/// query runs per group instead of one per item. Other op types insert
/// directly with a single `insert_many`, except `edit` which is deduplicated
/// per path within a 30-minute window (matching `log_activity`). Best-effort:
/// failures are logged via `tracing::warn!` and never propagated.
#[allow(clippy::too_many_arguments)]
pub async fn log_activity_batch(
    db: &DatabaseConnection,
    repo_id: &str,
    commit_id: &str,
    repo_name: &str,
    user_id: i32,
    items: Vec<ActivityItem>,
) {
    let now = chrono::Utc::now().timestamp();

    // Wiki repos are a separate product surface (知识库); skip their internal
    // file operations so they don't surface as ordinary library activities.
    if let Ok(Some(r)) = repo::Entity::find_by_id(repo_id).one(db).await
        && r.r#type == "wiki"
    {
        return;
    }

    // Non-aggregating ops (rename/move/recover) insert in one statement; edit
    // ops are deduplicated per path within a 30-minute window.
    let direct: Vec<&ActivityItem> = items
        .iter()
        .filter(|i| i.op_type != "create" && i.op_type != "delete")
        .collect();
    let edit_items: Vec<&ActivityItem> = direct
        .iter()
        .copied()
        .filter(|i| i.op_type == "edit")
        .collect();
    let other_direct: Vec<&ActivityItem> = direct
        .iter()
        .copied()
        .filter(|i| i.op_type != "edit")
        .collect();
    if !other_direct.is_empty() {
        let models: Vec<activity::ActiveModel> = other_direct
            .iter()
            .map(|i| to_active_model(repo_id, commit_id, repo_name, user_id, now, i))
            .collect();
        if let Err(e) = activity::Entity::insert_many(models).exec(db).await {
            tracing::warn!("Failed to batch insert activities: {e}");
        }
    }
    if !edit_items.is_empty() {
        dedup_edit_items(db, repo_id, commit_id, repo_name, user_id, now, &edit_items).await;
    }

    // create/delete aggregate into a recent batch row within 5 minutes,
    // grouped by (op_type, obj_type).
    let mut groups: HashMap<(&str, &str), Vec<&ActivityItem>> = HashMap::new();
    for item in items
        .iter()
        .filter(|i| i.op_type == "create" || i.op_type == "delete")
    {
        groups
            .entry((item.op_type, item.obj_type))
            .or_default()
            .push(item);
    }

    for ((op_type, obj_type), group_items) in groups {
        let batch_types = [op_type.to_string(), format!("batch_{op_type}")];
        let cutoff = now - 300; // 5-minute window

        let recent = activity::Entity::find()
            .filter(activity::Column::RepoId.eq(repo_id))
            .filter(activity::Column::UserId.eq(user_id))
            .filter(activity::Column::ObjType.eq(obj_type))
            .filter(activity::Column::OpType.is_in(batch_types))
            .filter(activity::Column::CreatedAt.gt(cutoff))
            .order_by_desc(activity::Column::CreatedAt)
            .one(db)
            .await;

        match recent {
            Ok(Some(row)) => {
                let mut detail_array = parse_detail_array(&row.detail);
                let mut remaining: Vec<&ActivityItem> = Vec::new();
                for item in &group_items {
                    if detail_array.len() >= ACTIVITY_MAX_AGGREGATE_ITEMS {
                        remaining.push(item);
                    } else {
                        detail_array
                            .push(serde_json::Value::Object(build_detail_map(item, repo_name)));
                    }
                }

                if detail_array.len() > parse_detail_array(&row.detail).len() {
                    let updated_detail =
                        serde_json::to_string(&detail_array).unwrap_or_else(|_| "[]".to_string());
                    let mut active: activity::ActiveModel = row.into();
                    active.op_type = Set(format!("batch_{op_type}"));
                    active.detail = Set(updated_detail);
                    active.created_at = Set(now);
                    if let Err(e) = activity::Entity::update(active).exec(db).await {
                        tracing::warn!("Failed to update aggregated activity: {e}");
                    }
                }

                if !remaining.is_empty() {
                    let models: Vec<activity::ActiveModel> = remaining
                        .iter()
                        .map(|i| to_active_model(repo_id, commit_id, repo_name, user_id, now, i))
                        .collect();
                    if let Err(e) = activity::Entity::insert_many(models).exec(db).await {
                        tracing::warn!("Failed to insert remaining aggregated activities: {e}");
                    }
                }
            }
            Ok(None) | Err(_) => {
                let models: Vec<activity::ActiveModel> = group_items
                    .iter()
                    .map(|i| to_active_model(repo_id, commit_id, repo_name, user_id, now, i))
                    .collect();
                if let Err(e) = activity::Entity::insert_many(models).exec(db).await {
                    tracing::warn!("Failed to insert aggregated activities: {e}");
                }
            }
        }
    }
}

/// Deduplicate `edit` activity items: for each path, if a recent edit row
/// (same repo/user/path within `EDIT_DEDUP_WINDOW`) exists, refresh its
/// timestamp; otherwise insert a new row.
async fn dedup_edit_items(
    db: &DatabaseConnection,
    repo_id: &str,
    commit_id: &str,
    repo_name: &str,
    user_id: i32,
    now: i64,
    items: &[&ActivityItem],
) {
    let cutoff = now - EDIT_DEDUP_WINDOW;
    let paths: Vec<&str> = items.iter().map(|i| i.path.as_str()).collect();

    // One query for all recent edit rows in this repo/user within the window,
    // ordered newest-first so the first row per path is the dedup target.
    let recent_rows = activity::Entity::find()
        .filter(activity::Column::RepoId.eq(repo_id))
        .filter(activity::Column::UserId.eq(user_id))
        .filter(activity::Column::OpType.eq("edit"))
        .filter(activity::Column::CreatedAt.gt(cutoff))
        .filter(activity::Column::Path.is_in(paths))
        .order_by_desc(activity::Column::CreatedAt)
        .all(db)
        .await
        .unwrap_or_default();

    let mut latest_by_path: HashMap<String, &activity::Model> = HashMap::new();
    for row in &recent_rows {
        latest_by_path.entry(row.path.clone()).or_insert(row);
    }

    let mut to_insert: Vec<activity::ActiveModel> = Vec::new();
    for item in items {
        if let Some(row) = latest_by_path.get(&item.path) {
            let mut active: activity::ActiveModel = (*row).clone().into();
            active.created_at = Set(now);
            if let Err(e) = activity::Entity::update(active).exec(db).await {
                tracing::warn!("Failed to update deduped edit activity: {e}");
            }
        } else {
            to_insert.push(to_active_model(
                repo_id, commit_id, repo_name, user_id, now, item,
            ));
        }
    }

    if !to_insert.is_empty()
        && let Err(e) = activity::Entity::insert_many(to_insert).exec(db).await
    {
        tracing::warn!("Failed to insert deduped edit activities: {e}");
    }
}

/// Look up a user's numeric ID by their email address.
///
/// Returns `None` if the user is not found or the query fails.
pub async fn user_id_by_email(db: &DatabaseConnection, email: &str) -> Option<i32> {
    crate::entity::user::Entity::find()
        .filter(crate::entity::user::Column::Email.eq(email))
        .one(db)
        .await
        .ok()
        .flatten()
        .map(|u| u.id)
}
