//! Permission checking — business rules for repo access control.
//!
//! These functions were moved from `infra::permission::repo` to the domain
//! layer because they encode business logic (who can read/write a repo),
//! not infrastructure concerns.  The underlying SQL queries use raw
//! `DatabaseConnection` rather than repository traits because the LEFT JOIN
//! pattern is a single query that doesn't map to a single entity.

use base::AppError;
use sea_orm::{ConnectionTrait, DatabaseBackend, DatabaseConnection, Statement};

/// Check if `user_id` has write (`rw`) permission on the repo.
///
/// The repo owner always has full access. Members are checked against
/// `repo_member.permission`. Non-members and read-only members are
/// rejected with `AppError::Forbidden`.
pub async fn check_repo_write_permission(
    db: &DatabaseConnection,
    repo_id: &str,
    user_id: i32,
) -> Result<(), AppError> {
    let row: Option<(i32, Option<String>)> = db
        .query_one(Statement::from_sql_and_values(
            DatabaseBackend::Sqlite,
            "SELECT r.owner_id, m.permission FROM repos r \
             LEFT JOIN repo_members m ON r.id = m.repo_id AND m.user_id = $1 \
             WHERE r.id = $2",
            vec![user_id.into(), repo_id.to_owned().into()],
        ))
        .await?
        .map(|r| {
            let owner_id: i32 = r.try_get("", "owner_id").unwrap_or(0);
            let permission: Option<String> = r.try_get("", "permission").ok();
            (owner_id, permission)
        });

    match row {
        None => Err(AppError::NotFound("repo not found".into())),
        Some((owner_id, _)) if owner_id == user_id => Ok(()),
        Some((_, Some(perm))) if perm == "rw" => Ok(()),
        _ => Err(AppError::Forbidden),
    }
}

/// Check if `user_id` has read permission on the repo.
///
/// The repo owner always has access. Any member (r or rw) has access.
/// Non-members are rejected with `AppError::Forbidden`.
pub async fn check_repo_read_permission(
    db: &DatabaseConnection,
    repo_id: &str,
    user_id: i32,
) -> Result<(), AppError> {
    let row: Option<(i32, Option<String>)> = db
        .query_one(Statement::from_sql_and_values(
            DatabaseBackend::Sqlite,
            "SELECT r.owner_id, m.permission FROM repos r \
             LEFT JOIN repo_members m ON r.id = m.repo_id AND m.user_id = $1 \
             WHERE r.id = $2",
            vec![user_id.into(), repo_id.to_owned().into()],
        ))
        .await?
        .map(|r| {
            let owner_id: i32 = r.try_get("", "owner_id").unwrap_or(0);
            let permission: Option<String> = r.try_get("", "permission").ok();
            (owner_id, permission)
        });

    match row {
        None => Err(AppError::NotFound("repo not found".into())),
        Some((owner_id, _)) if owner_id == user_id => Ok(()),
        Some((_, Some(_))) => Ok(()), // any membership grants read access
        _ => Err(AppError::Forbidden),
    }
}
