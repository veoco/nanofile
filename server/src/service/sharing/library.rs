//! Administrator-managed library sharing.
//!
//! Only an administrator who owns a library may publish it. A library is either
//! `private`, `shared` with the accounts listed in `repo_members`, or `public`
//! to every active account. Group sharing and folder-level sharing are not
//! modelled; the handlers reject those inputs instead of pretending to support
//! them.

use std::sync::Arc;

use base::error::AppError;
use infra::entity::{repo, user};

use crate::domain::repo_visibility::RepoVisibility;
use crate::repository::Repositories;

/// `r` or `rw`; the only two permissions a shared library can grant.
fn parse_permission(value: Option<&str>) -> Result<&'static str, AppError> {
    match value.map(str::trim).unwrap_or("r") {
        "" | "r" => Ok("r"),
        "rw" => Ok("rw"),
        _ => Err(AppError::BadRequest(
            "permission must be 'r' or 'rw'".into(),
        )),
    }
}

pub struct LibraryShareService {
    repos: Arc<Repositories>,
    origin: String,
}

impl LibraryShareService {
    pub fn new(repos: Arc<Repositories>, origin: String) -> Self {
        Self { repos, origin }
    }

    /// Load the library and require that `admin_id` is a real administrator and
    /// the library's owner.
    async fn owned_library(&self, repo_id: &str, admin_id: i32) -> Result<repo::Model, AppError> {
        let repo = self
            .repos
            .repo
            .find_by_id(repo_id)
            .await?
            .ok_or_else(|| AppError::NotFound("repo not found".into()))?;

        let admin = self
            .repos
            .user
            .find_by_id(admin_id)
            .await?
            .ok_or(AppError::Unauthorized)?;
        if !admin.is_admin || repo.owner_id != admin_id {
            return Err(AppError::Forbidden);
        }

        Ok(repo)
    }

    fn member_view(&self, user: &user::Model, permission: &str) -> serde_json::Value {
        serde_json::json!({
            "share_type": "user",
            "user_info": {
                "name": user.email,
                "nickname": user.nickname(),
                "contact_email": user.email,
                "avatar_url": format!(
                    "{}{}",
                    self.origin,
                    crate::service::user::primary_avatar_url(&user.email, 72)
                ),
            },
            "permission": permission,
            "is_admin": false,
        })
    }

    /// The user shares on a library, as the bare array the official client's
    /// share dialog parses.
    pub async fn list_members(
        &self,
        repo_id: &str,
        admin_id: i32,
    ) -> Result<Vec<serde_json::Value>, AppError> {
        let repo = self.owned_library(repo_id, admin_id).await?;
        let members = self.repos.member.find_by_repo_id(repo_id).await?;
        let user_ids: Vec<i32> = members
            .iter()
            .filter(|m| m.user_id != repo.owner_id)
            .map(|m| m.user_id)
            .collect();
        let users = self.repos.user.find_by_ids(&user_ids).await?;
        let by_id: std::collections::HashMap<i32, user::Model> =
            users.into_iter().map(|u| (u.id, u)).collect();
        let mut result = Vec::with_capacity(members.len());
        for member in members {
            if member.user_id == repo.owner_id {
                continue;
            }
            if let Some(user) = by_id.get(&member.user_id) {
                result.push(self.member_view(user, &member.permission));
            }
        }
        Ok(result)
    }

    async fn target_user(&self, email: &str) -> Result<user::Model, AppError> {
        let email = email.trim();
        if email.is_empty() {
            return Err(AppError::BadRequest("username is required".into()));
        }
        let user = self
            .repos
            .user
            .find_by_email(email)
            .await?
            .ok_or_else(|| AppError::NotFound("user not found".into()))?;
        if !user.is_active {
            return Err(AppError::BadRequest("user is inactive".into()));
        }
        Ok(user)
    }

    /// Add a member or replace their permission. A `private` library becomes
    /// `shared` in the same transaction.
    pub async fn add_member(
        &self,
        repo_id: &str,
        admin_id: i32,
        username: &str,
        permission: Option<&str>,
    ) -> Result<serde_json::Value, AppError> {
        let repo = self.owned_library(repo_id, admin_id).await?;
        if repo.encrypted != 0 {
            return Err(AppError::BadRequest(
                "encrypted libraries cannot be shared".into(),
            ));
        }
        if repo.visibility == RepoVisibility::Public.as_str() {
            return Err(AppError::BadRequest(
                "a public library cannot also list users; clear its public visibility first".into(),
            ));
        }
        let permission = parse_permission(permission)?;
        let target = self.target_user(username).await?;
        if target.id == repo.owner_id {
            return Err(AppError::BadRequest(
                "a library cannot be shared with its owner".into(),
            ));
        }

        self.repos
            .library_share
            .upsert_member(repo_id, target.id, permission)
            .await?;
        Ok(self.member_view(&target, permission))
    }

    pub async fn update_member(
        &self,
        repo_id: &str,
        admin_id: i32,
        username: &str,
        permission: Option<&str>,
    ) -> Result<serde_json::Value, AppError> {
        let repo = self.owned_library(repo_id, admin_id).await?;
        if repo.encrypted != 0 {
            return Err(AppError::BadRequest(
                "encrypted libraries cannot be shared".into(),
            ));
        }
        let permission = parse_permission(permission)?;
        let target = self.target_user(username).await?;
        if self
            .repos
            .member
            .find_by_repo_and_user(repo_id, target.id)
            .await?
            .is_none()
        {
            return Err(AppError::NotFound("share not found".into()));
        }
        self.repos
            .library_share
            .upsert_member(repo_id, target.id, permission)
            .await?;
        Ok(self.member_view(&target, permission))
    }

    /// Remove one member. Unknown targets and non-members are a no-op so the
    /// official client's "Leave share" action is idempotent.
    pub async fn remove_member(
        &self,
        repo_id: &str,
        admin_id: i32,
        username: &str,
    ) -> Result<(), AppError> {
        let repo = self.owned_library(repo_id, admin_id).await?;
        let target = self.target_user(username).await?;
        if target.id == repo.owner_id {
            return Err(AppError::BadRequest(
                "the owner cannot be removed from their library".into(),
            ));
        }
        self.repos
            .library_share
            .remove_member(repo_id, repo.owner_id, target.id)
            .await
    }

    /// A recipient removes their own grant. The owner cannot leave their own
    /// library and a non-member call is a no-op.
    pub async fn leave_share(&self, repo_id: &str, user_id: i32) -> Result<(), AppError> {
        let repo = self
            .repos
            .repo
            .find_by_id(repo_id)
            .await?
            .ok_or_else(|| AppError::NotFound("repo not found".into()))?;
        if repo.owner_id == user_id {
            return Err(AppError::BadRequest(
                "the owner cannot leave their own library".into(),
            ));
        }
        if self
            .repos
            .member
            .find_by_repo_and_user(repo_id, user_id)
            .await?
            .is_none()
        {
            return Ok(());
        }
        self.repos
            .library_share
            .remove_member(repo_id, repo.owner_id, user_id)
            .await
    }

    pub async fn set_visibility(
        &self,
        repo_id: &str,
        admin_id: i32,
        visibility: &str,
    ) -> Result<(), AppError> {
        let repo = self.owned_library(repo_id, admin_id).await?;
        let visibility = RepoVisibility::parse(visibility).ok_or_else(|| {
            AppError::BadRequest("visibility must be private, shared or public".into())
        })?;
        if repo.encrypted != 0 && visibility != RepoVisibility::Private {
            return Err(AppError::BadRequest(
                "encrypted libraries can only be private".into(),
            ));
        }
        let clear_members = visibility != RepoVisibility::Shared;
        self.repos
            .library_share
            .set_visibility(repo_id, visibility, clear_members)
            .await
    }

    /// Active users matching `q`, capped so an administrator's picker cannot
    /// enumerate the whole account directory in one request.
    pub async fn search_users(&self, q: &str) -> Result<Vec<serde_json::Value>, AppError> {
        const MAX_RESULTS: usize = 50;
        let q = q.trim();
        if q.is_empty() {
            return Ok(Vec::new());
        }
        // The repository query uses `LIKE ... ESCAPE '\'`.
        let escaped = q
            .replace('\\', "\\\\")
            .replace('%', "\\%")
            .replace('_', "\\_");
        let users = self
            .repos
            .user
            .find_by_email_like(&format!("%{escaped}%"))
            .await?;
        Ok(users
            .into_iter()
            .filter(|u| u.is_active)
            .take(MAX_RESULTS)
            .map(|u| {
                serde_json::json!({
                    "email": u.email,
                    "name": u.nickname(),
                    "contact_email": u.email,
                    "avatar_url": format!(
                        "{}{}",
                        self.origin,
                        crate::service::user::primary_avatar_url(&u.email, 72)
                    ),
                })
            })
            .collect())
    }
}
