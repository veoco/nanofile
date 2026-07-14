//! Permission checking — business rules for repo access control.
//!
//! These functions encode pure business logic (who can read/write a repo).
//! The actual data retrieval is delegated to `MemberRepository` via trait,
//! keeping this module free of infrastructure concerns.

use crate::repository::member::MemberRepository;
use base::AppError;

/// Check if `user_id` has write (`rw`) permission on the repo.
///
/// The repo owner always has full access. Members are checked against
/// `repo_member.permission`. Non-members and read-only members are
/// rejected with `AppError::Forbidden`.
pub async fn check_repo_write_permission(
    member_repo: &dyn MemberRepository,
    repo_id: &str,
    user_id: i32,
) -> Result<(), AppError> {
    let row = member_repo
        .find_repo_owner_and_permission(repo_id, user_id)
        .await?;

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
    member_repo: &dyn MemberRepository,
    repo_id: &str,
    user_id: i32,
) -> Result<(), AppError> {
    let row = member_repo
        .find_repo_owner_and_permission(repo_id, user_id)
        .await?;

    match row {
        None => Err(AppError::NotFound("repo not found".into())),
        Some((owner_id, _)) if owner_id == user_id => Ok(()),
        Some((_, Some(_))) => Ok(()), // any membership grants read access
        _ => Err(AppError::Forbidden),
    }
}
