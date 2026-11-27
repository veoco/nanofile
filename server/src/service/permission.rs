//! Authorization checks over the repository ports.
//!
//! These are policy questions ("may this account read/write this library?") whose
//! answer needs the membership row, so they live in the service layer where a
//! repository is available; `domain::permission` keeps only the pure `RepoScope`
//! value the checks and listings pass around.

use crate::domain::permission::RepoScope;
use crate::domain::repo_visibility::RepoVisibility;
use crate::repository::Repositories;
use crate::repository::member::MemberRepository;
use base::error::AppError;
/// Repo ids `user_id` can currently access: owned libraries, every membership
/// grant, and every `public` library, narrowed by `scope`. Listing endpoints use
/// it so a row referring to a library the user has since been removed from cannot
/// keep leaking its name, paths or modification times.
pub async fn accessible_repo_ids(
    repos: &Repositories,
    user_id: i32,
    scope: &RepoScope,
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
    for public in repos
        .repo
        .find_by_visibility(RepoVisibility::Public.as_str())
        .await?
    {
        ids.insert(public.id);
    }
    Ok(scope.intersect(ids))
}

/// Write permission check that also returns the library owner's id.
///
/// Quota accounting charges the owner, so a caller that reserves bytes per block
/// takes the id from the permission row it has already read instead of looking the
/// owner up again for every block (`repos.repo_owner_id`).
pub async fn repo_write_owner(
    member_repo: &dyn MemberRepository,
    repo_id: &str,
    user_id: i32,
) -> Result<i32, AppError> {
    let row = member_repo
        .find_repo_owner_and_permission(repo_id, user_id)
        .await?;

    match row {
        None => Err(AppError::NotFound("repo not found".into())),
        Some((owner_id, _, _)) if owner_id == user_id => Ok(owner_id),
        Some((owner_id, Some(perm), _)) if perm == "rw" => Ok(owner_id),
        _ => Err(AppError::Forbidden),
    }
}

/// Check if `user_id` has write (`rw`) permission on the repo. The owner always
/// has full access; members are checked against `repo_member.permission`, and
/// non-members and read-only members are rejected with `AppError::Forbidden`.
pub async fn check_repo_write_permission(
    member_repo: &dyn MemberRepository,
    repo_id: &str,
    user_id: i32,
) -> Result<(), AppError> {
    repo_write_owner(member_repo, repo_id, user_id)
        .await
        .map(|_| ())
}

/// Check if `user_id` is the owner of the repo. Member management is owner-only:
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
        Some((owner_id, _, _)) if owner_id == user_id => Ok(()),
        _ => Err(AppError::Forbidden),
    }
}

/// The caller's permission on the repo as seahub spells it: `"rw"` for the owner
/// and rw members, `"r"` for read-only members and for every reader of a
/// `public` library — the same strings seahub puts in a listing's `dir_perm`
/// header, each entry's `permission` and v2.1's `user_perm`.
///
/// `Ok(None)` means the user has neither a membership grant nor public access,
/// while a missing repo is `NotFound`: the query is a `repos` LEFT JOIN, so the
/// two are distinguishable.
pub async fn repo_permission(
    member_repo: &dyn MemberRepository,
    repo_id: &str,
    user_id: i32,
) -> Result<Option<&'static str>, AppError> {
    let row = member_repo
        .find_repo_owner_and_permission(repo_id, user_id)
        .await?;

    match row {
        None => Err(AppError::NotFound("repo not found".into())),
        Some((owner_id, _, _)) if owner_id == user_id => Ok(Some("rw")),
        Some((_, Some(perm), _)) if perm == "rw" => Ok(Some("rw")),
        Some((_, Some(_), _)) => Ok(Some("r")),
        Some((_, None, visibility)) if visibility == RepoVisibility::Public.as_str() => {
            Ok(Some("r"))
        }
        Some((_, None, _)) => Ok(None),
    }
}

/// The caller's permission when only owned or explicitly shared libraries count.
///
/// Public libraries are readable by everyone but are administrator-published
/// content; an anonymous share link created by any logged-in reader would turn
/// that into a redistribution channel. This is the gate for link creation.
pub async fn repo_member_permission(
    member_repo: &dyn MemberRepository,
    repo_id: &str,
    user_id: i32,
) -> Result<Option<&'static str>, AppError> {
    let row = member_repo
        .find_repo_owner_and_permission(repo_id, user_id)
        .await?;

    match row {
        None => Err(AppError::NotFound("repo not found".into())),
        Some((owner_id, _, _)) if owner_id == user_id => Ok(Some("rw")),
        Some((_, Some(perm), _)) if perm == "rw" => Ok(Some("rw")),
        Some((_, Some(_), _)) => Ok(Some("r")),
        Some((_, None, _)) => Ok(None),
    }
}

/// Check if `user_id` has read permission on the repo. The owner always has
/// access, any member (r or rw) has access, and a `public` library is readable by
/// every authenticated account; non-members of a private/shared library get
/// `Forbidden`.
pub async fn check_repo_read_permission(
    member_repo: &dyn MemberRepository,
    repo_id: &str,
    user_id: i32,
) -> Result<(), AppError> {
    match repo_permission(member_repo, repo_id, user_id).await? {
        Some(_) => Ok(()),
        None => Err(AppError::Forbidden),
    }
}

/// [`check_repo_read_permission`] without the public fallback: the caller must be
/// the owner or an explicit member.
pub async fn check_repo_member_read_permission(
    member_repo: &dyn MemberRepository,
    repo_id: &str,
    user_id: i32,
) -> Result<(), AppError> {
    match repo_member_permission(member_repo, repo_id, user_id).await? {
        Some(_) => Ok(()),
        None => Err(AppError::Forbidden),
    }
}
