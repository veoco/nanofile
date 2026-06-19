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

use crate::entity::{activity, repo};

/// Maximum number of items in a batch-aggregated activity detail array.
const ACTIVITY_MAX_AGGREGATE_ITEMS: usize = 200;

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
) {
    let now = chrono::Utc::now().timestamp();

    // Best-effort: look up repo name and head commit ID.
    let (commit_id, repo_name) = match repo::Entity::find_by_id(repo_id).one(db).await {
        Ok(Some(r)) => (r.head_commit_id.unwrap_or_default(), r.name.clone()),
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

/// Look up a user's numeric ID by their email address.
///
/// Returns `None` if the user is not found or the query fails.
pub async fn user_id_by_email(db: &DatabaseConnection, email: &str) -> Option<i32> {
    use sea_orm::QueryFilter;
    crate::entity::user::Entity::find()
        .filter(crate::entity::user::Column::Email.eq(email))
        .one(db)
        .await
        .ok()
        .flatten()
        .map(|u| u.id)
}
