//! Permission checking — business rules for repo access control.
//!
//! These functions encode pure business logic (who can read/write a repo).
//! The actual data retrieval is delegated to `MemberRepository` via trait,
//! keeping this module free of infrastructure concerns.

use crate::repository::Repositories;
use crate::repository::member::MemberRepository;
use base::AppError;

/// Repo ids `user_id` can currently access: the libraries they own plus every
/// library a membership row grants them.
///
/// Used by the listing endpoints (starred items, activities, search) to
/// intersect a user-specific table with live access, so a row that refers to a
/// library the user has since been removed from does not keep leaking its name,
/// paths or modification times.
pub async fn accessible_repo_ids(
    repos: &Repositories,
    user_id: i32,
) -> Result<std::collections::HashSet<String>, AppError> {
    let mut ids: std::collections::HashSet<String> = repos
        .repo
        .find_by_owner_id(user_id)
        .await?
        .into_iter()
        .map(|r| r.id)
        .collect();
    for member in repos.member.find_by_user_id(user_id).await? {
        ids.insert(member.repo_id);
    }
    Ok(ids)
}

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

/// Check if `user_id` is the owner of the repo.
///
/// Member management (sharing / modifying / removing members) is owner-only:
/// an rw member must not be able to impersonate the owner. Non-owners are
/// rejected with `AppError::Forbidden`.
pub async fn check_repo_owner(
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
