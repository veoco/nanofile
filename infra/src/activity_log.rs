/// File activity event logger.
///
/// One entry point records user-initiated file operations (create, rename, move,
/// delete, edit) into the `activities` table: op type, path, user, the repo's HEAD
/// commit and a `detail` JSON matching seafevents' `Activity.detail`.
///
/// `create`/`delete` aggregate within a 5-minute window into one `batch_create` /
/// `batch_delete` row (matching seafevents' `save_user_activity`). All errors are
/// logged and never propagated: activity logging must not break the caller.
use sea_orm::{ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter, QueryOrder, Set};
use std::collections::HashMap;

use crate::entity::{activity, repo};

/// Maximum number of items in a batch-aggregated activity detail array.
const ACTIVITY_MAX_AGGREGATE_ITEMS: usize = 200;

/// Window within which repeated edits of one path collapse into a row (seafevents).
const EDIT_DEDUP_WINDOW: i64 = 1800;

/// Log a file operation activity.
///
/// `op_type` (`create`/`delete`/`edit`/`rename`/`move`/`recover`) and `obj_type`
/// (`file`/`dir`) follow seafevents' constants.
///
/// `create`/`delete` aggregate into a recent batch row within a 5-minute window
/// (same repo, user and obj_type), promoting `op_type` to `batch_create` /
/// `batch_delete` and appending the detail item. Other ops are never aggregated.
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

    let (commit_id, repo_name) = match repo::Entity::find_by_id(repo_id).one(db).await {
        Ok(Some(r)) => (r.head_commit_id.unwrap_or_default(), r.name.clone()),
        _ => (String::new(), String::new()),
    };

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

    // Only create/delete aggregate (seafevents' BATCH_AGGREGATE_OP_TYPES).
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
            // A recent batch row: try to append to it.
            if let Ok(current_detail) = serde_json::from_str::<serde_json::Value>(&recent.detail) {
                let mut detail_array: Vec<serde_json::Value> = match &current_detail {
                    serde_json::Value::Array(arr) => arr.clone(),
                    serde_json::Value::Object(_) => {
                        // Extract only the allowed detail keys, so the first item in a batch array has the
                        // same shape as subsequently appended items.
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

    // A repeated edit within the window only refreshes the row's timestamp.
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

/// Lightweight entry for batched logging, kept in `infra` so the sync commit path
/// can log many items without a server-crate dependency.
pub struct ActivityItem {
    pub op_type: &'static str,
    pub obj_type: &'static str,
    pub path: String,
    pub old_path: Option<String>,
    pub size: Option<i64>,
    pub obj_id: Option<String>,
    /// The change came out of a server-side merge rather than a device writing this
    /// path; stored as `detail.merge` so the activity page can say so.
    pub merged: bool,
    /// The merge kept a path one side deleted or renamed while the other changed it:
    /// no device saved to this path. Stored as `detail.merge_kept`.
    pub merge_kept: bool,
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
    if item.merged {
        detail_map.insert("merge".to_string(), serde_json::json!(true));
    }
    if item.merge_kept {
        detail_map.insert("merge_kept".to_string(), serde_json::json!(true));
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

/// Parse a stored `detail` into items, reshaping a lone object into a one-element
/// array using only the allowed keys.
fn parse_detail_array(detail: &str) -> Vec<serde_json::Value> {
    match serde_json::from_str::<serde_json::Value>(detail) {
        Ok(serde_json::Value::Array(arr)) => arr,
        Ok(serde_json::Value::Object(obj)) => {
            let allowed_keys: [&str; 8] = [
                "obj_id",
                "size",
                "old_path",
                "repo_name",
                "old_repo_name",
                "path",
                "merge",
                "merge_kept",
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

/// Batch-log activity on hot commit paths. `commit_id` and `repo_name` are supplied
/// once per commit, eliminating the per-item repo lookup (N+1).
///
/// `create`/`delete` keep the 5-minute aggregation, grouped by `(op_type, obj_type)`
/// so one query runs per group; other types insert with `insert_many`, except `edit`
/// which dedups per path in a 30-minute window. Failures are logged, never propagated.
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

    // Non-aggregating ops insert in one statement; edit dedups per path in 30 minutes.
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

    // create/delete aggregate into a recent batch row within 5 minutes, grouped by type.
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

/// Dedup `edit` items: refresh the timestamp of a recent row for the same path, or
/// insert a new one.
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

    // One newest-first query covers every path's dedup target.
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
