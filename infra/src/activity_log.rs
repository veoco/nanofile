/// File activity event logger.
///
/// One entry point records user-initiated file operations (create, rename, move,
/// delete, edit) into the `activities` table. Each activity is a summary row;
/// the affected objects live in `activity_items`. That keeps one upload of a
/// large directory to one summary per operation instead of one row per file,
/// while the exact item count stays available and the detail payload can be
/// bounded independently.
///
/// `create`/`delete` still aggregate single (non-commit) calls within a 5-minute
/// window into one `batch_create` / `batch_delete` summary, matching seafevents'
/// `save_user_activity`. The sync commit path groups by `commit_id` instead: a
/// commit is a deterministic operation boundary, so it never needs a clock
/// window. All errors are logged and never propagated: activity logging must not
/// break the caller.
use sea_orm::{
    ColumnTrait, ConnectionTrait, DatabaseConnection, EntityTrait, QueryFilter, QueryOrder, Set,
    TransactionTrait,
};
use std::collections::HashMap;

use crate::entity::{activity, activity_item, repo};

/// Window within which repeated edits of one path collapse into a row (seafevents).
const EDIT_DEDUP_WINDOW: i64 = 1800;

/// A single call that the 5-minute aggregation may append to.
const GROUP_SESSION: &str = "session";

/// A summary produced from one sync commit; never appended to by a Web upload.
const GROUP_COMMIT: &str = "commit";

/// Rows per `activity_items` insert. Seven bound columns per row keeps this well
/// under SQLite's variable limit, including the classic 999-variable builds.
const ITEM_INSERT_BATCH: usize = 100;

/// Serializes the read-modify-write of an aggregated summary.
///
/// `create`/`delete` append one item to the newest summary inside a 5-minute
/// window: read the summary, write the item, bump `item_count`. Two concurrent
/// calls for the same user and library (two browser tabs, or a delete next to a
/// create) could otherwise both read the same count and drop one increment.
/// SQLite serializes statements, not the read-then-write pair, so this needs a
/// lock; the server is a single process and SQLite is its single writer, which is
/// why an in-process mutex is enough.
static AGGREGATE_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

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
    /// path; stored as `merge` so the activity page can say so.
    pub merged: bool,
    /// The merge kept a path one side deleted or renamed while the other changed it:
    /// no device saved to this path. Stored as `merge_kept` so the activity page can
    /// say so.
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

fn item_model_from_parts(
    activity_id: i32,
    path: &str,
    old_path: Option<&str>,
    size: Option<i64>,
    obj_id: Option<&str>,
    merged: bool,
    merge_kept: bool,
) -> activity_item::ActiveModel {
    activity_item::ActiveModel {
        id: sea_orm::NotSet,
        activity_id: Set(activity_id),
        path: Set(path.to_string()),
        old_path: Set(old_path.map(str::to_string)),
        size: Set(size),
        obj_id: Set(obj_id.map(str::to_string)),
        merge: Set(merged),
        merge_kept: Set(merge_kept),
    }
}

fn item_model(activity_id: i32, item: &ActivityItem) -> activity_item::ActiveModel {
    item_model_from_parts(
        activity_id,
        &item.path,
        item.old_path.as_deref(),
        item.size,
        item.obj_id.as_deref(),
        item.merged,
        item.merge_kept,
    )
}

/// Insert a summary row and its items in one go. The items' `activity_id` is
/// filled in after the summary insert, and the insert is chunked so a directory
/// with thousands of files does not exceed SQLite's bound-variable limit.
async fn insert_summary_with_items<C: ConnectionTrait>(
    conn: &C,
    summary: activity::ActiveModel,
    items: Vec<activity_item::ActiveModel>,
) -> Result<(), sea_orm::DbErr> {
    let result = activity::Entity::insert(summary).exec(conn).await?;
    let activity_id = result.last_insert_id;
    let mut items = items.into_iter();
    loop {
        let chunk: Vec<_> = items.by_ref().take(ITEM_INSERT_BATCH).collect();
        if chunk.is_empty() {
            break;
        }
        let chunk: Vec<_> = chunk
            .into_iter()
            .map(|mut item| {
                item.activity_id = Set(activity_id);
                item
            })
            .collect();
        activity_item::Entity::insert_many(chunk).exec(conn).await?;
    }
    Ok(())
}

fn summary_detail_json(first: &ActivityItem, repo_name: &str, items: &[&ActivityItem]) -> String {
    let mut detail_map = build_detail_map(first, repo_name);
    if items.iter().any(|item| item.merged) {
        detail_map.insert("merge".to_string(), serde_json::json!(true));
    }
    if items.iter().any(|item| item.merge_kept) {
        detail_map.insert("merge_kept".to_string(), serde_json::json!(true));
    }
    serde_json::to_string(&detail_map).unwrap_or_else(|_| "{}".to_string())
}

/// Log a file operation activity.
///
/// `op_type` (`create`/`delete`/`edit`/`rename`/`move`/`recover`) and `obj_type`
/// (`file`/`dir`) follow seafevents' constants.
///
/// `create`/`delete` aggregate into a recent summary row within a 5-minute window
/// (same repo, user and obj_type), promoting `op_type` to `batch_create` /
/// `batch_delete` and adding one item. Other ops are never aggregated. The
/// summary and its item are written in one transaction.
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
    let detail_json = serde_json::to_string(&detail_map).unwrap_or_else(|_| "{}".to_string());

    // Only create/delete aggregate (seafevents' BATCH_AGGREGATE_OP_TYPES).
    if op_type == "create" || op_type == "delete" {
        // Held across the read, the append and the write, which is the whole point.
        let _aggregate = AGGREGATE_LOCK.lock().await;
        let batch_op_type = format!("batch_{op_type}");
        let recent = activity::Entity::find()
            .filter(activity::Column::RepoId.eq(repo_id))
            .filter(activity::Column::UserId.eq(user_id))
            .filter(activity::Column::ObjType.eq(obj_type))
            .filter(activity::Column::OpType.is_in([op_type.to_string(), batch_op_type]))
            .filter(activity::Column::GroupKind.eq(GROUP_SESSION))
            .filter(activity::Column::ItemCount.gt(0))
            .filter(activity::Column::CreatedAt.gt(now - 300))
            .order_by_desc(activity::Column::CreatedAt)
            .one(db)
            .await;

        if let Ok(Some(recent)) = recent {
            let next_count = recent.item_count + 1;
            let recent_id = recent.id;
            match db.begin().await {
                Ok(txn) => {
                    let inserted = activity_item::Entity::insert(item_model_from_parts(
                        recent_id, path, old_path, size, obj_id, false, false,
                    ))
                    .exec(&txn)
                    .await;

                    let mut active: activity::ActiveModel = recent.into();
                    active.op_type = Set(format!("batch_{op_type}"));
                    active.item_count = Set(next_count);
                    active.created_at = Set(now);
                    let updated = activity::Entity::update(active).exec(&txn).await;

                    if inserted.is_ok() && updated.is_ok() && txn.commit().await.is_ok() {
                        return;
                    }
                    tracing::warn!("Failed to aggregate activity ({op_type} {path})");
                }
                Err(e) => {
                    tracing::warn!("Failed to begin activity aggregation ({op_type} {path}): {e}");
                }
            }
        }
    }

    // A repeated edit within the window only refreshes the row's timestamp. The
    // item table keeps the first edit's snapshot; edit history proper lives in
    // the file history, not in the activity feed.
    if op_type == "edit" {
        let cutoff = now - EDIT_DEDUP_WINDOW;
        if let Ok(Some(recent)) = activity::Entity::find()
            .filter(activity::Column::RepoId.eq(repo_id))
            .filter(activity::Column::UserId.eq(user_id))
            .filter(activity::Column::Path.eq(path))
            .filter(activity::Column::OpType.eq("edit"))
            .filter(activity::Column::GroupKind.eq(GROUP_SESSION))
            .filter(activity::Column::ItemCount.gt(0))
            .filter(activity::Column::CreatedAt.gt(cutoff))
            .order_by_desc(activity::Column::CreatedAt)
            .one(db)
            .await
        {
            let mut active: activity::ActiveModel = recent.into();
            active.created_at = Set(now);
            if let Err(e) = activity::Entity::update(active).exec(db).await {
                tracing::warn!("Failed to log deduped edit activity ({path}): {e}");
            }
            return;
        }
    }

    let summary = activity::ActiveModel {
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
        item_count: Set(1),
        group_kind: Set(GROUP_SESSION.to_string()),
    };
    let item = item_model_from_parts(0, path, old_path, size, obj_id, false, false);

    match db.begin().await {
        Ok(txn) => {
            if let Err(e) = insert_summary_with_items(&txn, summary, vec![item]).await {
                tracing::warn!("Failed to log activity ({op_type} {path}): {e}");
                return;
            }
            if let Err(e) = txn.commit().await {
                tracing::warn!("Failed to commit activity ({op_type} {path}): {e}");
            }
        }
        Err(e) => tracing::warn!("Failed to begin activity transaction ({op_type} {path}): {e}"),
    }
}

/// Batch-log activity on hot commit paths. `commit_id` and `repo_name` are supplied
/// once per commit.
///
/// `create`/`delete` group by `(op_type, obj_type)` **within this commit** and
/// write one summary row plus its items. The commit is the operation boundary, so
/// there is no 5-minute window and two commits never merge just because they
/// arrived close together. A single item keeps the plain `create`/`delete` op
/// type; a group uses `batch_create`/`batch_delete`.
///
/// `edit` keeps the per-path 30-minute dedup, and `rename`/`move`/`recover` stay
/// one row per item: those operations are not batch-shaped in the seahub API.
/// Failures are logged, never propagated.
#[allow(clippy::too_many_arguments)]
pub async fn log_activity_batch(
    db: &DatabaseConnection,
    repo_id: &str,
    commit_id: &str,
    repo_name: &str,
    user_id: i32,
    items: Vec<ActivityItem>,
) {
    if items.is_empty() {
        return;
    }
    let now = chrono::Utc::now().timestamp();

    let txn = match db.begin().await {
        Ok(txn) => txn,
        Err(e) => {
            tracing::warn!("Failed to begin activity batch for {commit_id}: {e}");
            return;
        }
    };

    let mut groups: HashMap<(&str, &str), Vec<&ActivityItem>> = HashMap::new();
    let mut edits: Vec<&ActivityItem> = Vec::new();
    let mut others: Vec<&ActivityItem> = Vec::new();
    for item in &items {
        match item.op_type {
            "create" | "delete" => groups
                .entry((item.op_type, item.obj_type))
                .or_default()
                .push(item),
            "edit" => edits.push(item),
            _ => others.push(item),
        }
    }

    // One summary per (op_type, obj_type) group in the commit. No cross-commit
    // merge: a commit is the unit the user performed.
    for ((op_type, obj_type), group) in groups {
        let summary_op = if group.len() > 1 {
            format!("batch_{op_type}")
        } else {
            op_type.to_string()
        };
        let first = group[0];
        let summary = activity::ActiveModel {
            id: sea_orm::NotSet,
            repo_id: Set(repo_id.to_string()),
            commit_id: Set(commit_id.to_string()),
            op_type: Set(summary_op),
            obj_type: Set(obj_type.to_string()),
            path: Set(first.path.clone()),
            old_path: Set(first.old_path.clone()),
            user_id: Set(user_id),
            created_at: Set(now),
            detail: Set(summary_detail_json(first, repo_name, &group)),
            item_count: Set(group.len() as i32),
            group_kind: Set(GROUP_COMMIT.to_string()),
        };
        let item_models: Vec<_> = group.iter().map(|item| item_model(0, item)).collect();
        if let Err(e) = insert_summary_with_items(&txn, summary, item_models).await {
            tracing::warn!("Failed to log activity batch for {commit_id}: {e}");
            return;
        }
    }

    // Edits keep the per-path 30-minute dedup, matching the single-event path.
    let mut latest_by_path: HashMap<String, i32> = HashMap::new();
    for chunk in edits.chunks(500) {
        let paths: Vec<&str> = chunk.iter().map(|item| item.path.as_str()).collect();
        let recent = activity::Entity::find()
            .filter(activity::Column::RepoId.eq(repo_id))
            .filter(activity::Column::UserId.eq(user_id))
            .filter(activity::Column::OpType.eq("edit"))
            .filter(activity::Column::GroupKind.eq(GROUP_COMMIT))
            .filter(activity::Column::ItemCount.gt(0))
            .filter(activity::Column::CreatedAt.gt(now - EDIT_DEDUP_WINDOW))
            .filter(activity::Column::Path.is_in(paths))
            .order_by_desc(activity::Column::CreatedAt)
            .all(&txn)
            .await;
        match recent {
            Ok(rows) => {
                for row in rows {
                    latest_by_path.entry(row.path.clone()).or_insert(row.id);
                }
            }
            Err(e) => tracing::warn!("Failed to look up deduped edit activities: {e}"),
        }
    }

    for item in edits {
        // A merge-produced edit must leave a marker; do not fold it into an
        // earlier plain edit of the same path.
        if !item.merged
            && !item.merge_kept
            && let Some(activity_id) = latest_by_path.get(&item.path)
        {
            let active = activity::ActiveModel {
                id: Set(*activity_id),
                created_at: Set(now),
                ..Default::default()
            };
            if let Err(e) = activity::Entity::update(active).exec(&txn).await {
                tracing::warn!("Failed to update deduped edit activity: {e}");
            }
            continue;
        }

        let summary = activity::ActiveModel {
            id: sea_orm::NotSet,
            repo_id: Set(repo_id.to_string()),
            commit_id: Set(commit_id.to_string()),
            op_type: Set(item.op_type.to_string()),
            obj_type: Set(item.obj_type.to_string()),
            path: Set(item.path.clone()),
            old_path: Set(item.old_path.clone()),
            user_id: Set(user_id),
            created_at: Set(now),
            detail: Set(summary_detail_json(item, repo_name, &[item])),
            item_count: Set(1),
            group_kind: Set(GROUP_COMMIT.to_string()),
        };
        if let Err(e) = insert_summary_with_items(&txn, summary, vec![item_model(0, item)]).await {
            tracing::warn!("Failed to log edit activity for {commit_id}: {e}");
            return;
        }
    }

    // Rename / move / recover are one row per item in the seahub feed.
    for item in others {
        let summary = activity::ActiveModel {
            id: sea_orm::NotSet,
            repo_id: Set(repo_id.to_string()),
            commit_id: Set(commit_id.to_string()),
            op_type: Set(item.op_type.to_string()),
            obj_type: Set(item.obj_type.to_string()),
            path: Set(item.path.clone()),
            old_path: Set(item.old_path.clone()),
            user_id: Set(user_id),
            created_at: Set(now),
            detail: Set(summary_detail_json(item, repo_name, &[item])),
            item_count: Set(1),
            group_kind: Set(GROUP_COMMIT.to_string()),
        };
        if let Err(e) = insert_summary_with_items(&txn, summary, vec![item_model(0, item)]).await {
            tracing::warn!(
                "Failed to log {:?} activity for {commit_id}: {e}",
                item.op_type
            );
            return;
        }
    }

    if let Err(e) = txn.commit().await {
        tracing::warn!("Failed to commit activity batch for {commit_id}: {e}");
    }
}
