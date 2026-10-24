//! Pure business rules for repo access control; data retrieval is delegated to
//! `MemberRepository`, so this module stays free of infrastructure concerns.

use crate::repository::Repositories;
use crate::repository::member::MemberRepository;
use base::AppError;

/// The libraries an account-wide query may return. A session (or a key bound to
/// every library) is unrestricted; a bound key narrows every listing to its own
/// libraries, so `starred`, `activities`, `search` and the link lists cannot
/// describe libraries it was not given.
#[derive(Clone, Debug, Default)]
pub struct RepoScope(Option<std::collections::HashSet<String>>);

impl RepoScope {
    /// No narrowing: every library the account can reach.
    pub fn all() -> Self {
        Self(None)
    }

    /// Narrow to an explicit set of library ids.
    pub fn only(ids: impl IntoIterator<Item = String>) -> Self {
        Self(Some(ids.into_iter().collect()))
    }

    pub fn allows(&self, repo_id: &str) -> bool {
        match &self.0 {
            None => true,
            Some(allowed) => allowed.contains(repo_id),
        }
    }

    pub fn intersect(
        &self,
        ids: std::collections::HashSet<String>,
    ) -> std::collections::HashSet<String> {
        match &self.0 {
            None => ids,
            Some(allowed) => ids.into_iter().filter(|id| allowed.contains(id)).collect(),
        }
    }

    /// Whether this scope imposes any restriction at all.
    pub fn is_unrestricted(&self) -> bool {
        self.0.is_none()
    }
}

/// Repo ids `user_id` can currently access: owned libraries plus every membership
/// grant, narrowed by `scope`. Listing endpoints use it so a row referring to a
/// library the user has since been removed from cannot keep leaking its name,
/// paths or modification times.
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
    Ok(scope.intersect(ids))
}

/// Check if `user_id` has write (`rw`) permission on the repo. The owner always
/// has full access; members are checked against `repo_member.permission`, and
/// non-members and read-only members are rejected with `AppError::Forbidden`.
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
        Some((owner_id, _)) if owner_id == user_id => Ok(()),
        _ => Err(AppError::Forbidden),
    }
}

/// The caller's permission on the repo as seahub spells it: `"rw"` for the owner
/// and rw members, `"r"` for any other member — the same strings seahub puts in a
/// listing's `dir_perm` header, each entry's `permission` and v2.1's `user_perm`.
///
/// `Ok(None)` means the user has no membership grant, while a missing repo is
/// `NotFound`: the query is a `repos` LEFT JOIN, so the two are distinguishable.
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
        Some((owner_id, _)) if owner_id == user_id => Ok(Some("rw")),
        Some((_, Some(perm))) if perm == "rw" => Ok(Some("rw")),
        Some((_, Some(_))) => Ok(Some("r")),
        Some((_, None)) => Ok(None),
    }
}

/// Check if `user_id` has read permission on the repo. The owner always has
/// access and any member (r or rw) has access; non-members get `Forbidden`.
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
