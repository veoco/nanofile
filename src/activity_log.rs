/// File activity event logger.
///
/// Provides a single entry point for recording user-initiated file operations
/// (create, rename, move, delete, edit) into the `activities` table.  Each
/// record captures the operation type, affected path, acting user, and the
/// repo's current HEAD commit ID at the time of the operation.
use sea_orm::{DatabaseConnection, EntityTrait, Set};

use crate::entity::{activity, repo};

/// Log a file operation activity.
///
/// Retrieves the repo's current head commit ID from the DB and inserts an
/// `activities` row.  Errors are logged via `tracing::warn!` but **never
/// propagated** — activity logging is best-effort and must not break the
/// calling operation.
pub async fn log_activity(
    db: &DatabaseConnection,
    repo_id: &str,
    op_type: &str,
    obj_type: &str,
    path: &str,
    user_id: i32,
    old_path: Option<&str>,
) {
    let now = chrono::Utc::now().timestamp();

    // Best-effort: look up the repo's head commit ID.
    let commit_id = match repo::Entity::find_by_id(repo_id).one(db).await {
        Ok(Some(r)) => r.head_commit_id.unwrap_or_default(),
        _ => String::new(),
    };

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
    use sea_orm::ColumnTrait;
    use sea_orm::QueryFilter;
    crate::entity::user::Entity::find()
        .filter(crate::entity::user::Column::Email.eq(email))
        .one(db)
        .await
        .ok()
        .flatten()
        .map(|u| u.id)
}
