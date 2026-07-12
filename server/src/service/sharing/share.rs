use sea_orm::{ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter, Set};
use serde::Serialize;
use std::sync::Arc;

use crate::Config;
use crate::fs::core::tree::{read_fs_dir_data, resolve_fs_id};
use crate::notification::events::FolderPermEvent;
use crate::repository::Repositories;
use crate::service::auth::password::hash_password;
use crate::service::auth::token::generate_share_link_token;
use base::error::AppError;
use infra::entity::share_link;

/// Resolve the s_type ("f" or "d") for a path in a repo by walking the FS tree.
pub async fn resolve_entry_type_raw(
    repos: &Repositories,
    repo_id: &str,
    path: &str,
) -> Result<String, AppError> {
    if path == "/" || path.is_empty() {
        return Ok("d".to_string());
    }

    let repo_model = repos
        .repo
        .find_by_id(repo_id)
        .await?
        .ok_or_else(|| AppError::NotFound("repo not found".into()))?;
    let head_commit_id = repo_model
        .head_commit_id
        .ok_or_else(|| AppError::BadRequest("repo has no commits".into()))?;
    let head_commit = repos
        .commit
        .find_by_repo_and_commit_id(repo_id, &head_commit_id)
        .await?
        .ok_or_else(|| AppError::Internal("head commit not found".into()))?;

    // Resolve the parent directory to find the entry's mode
    let (parent_path, entry_name) = path.rsplit_once('/').unwrap_or(("/", path));

    let parent_fs_id = if parent_path.is_empty() {
        head_commit.root_id.clone()
    } else {
        resolve_fs_id(repos, repo_id, &head_commit.root_id, parent_path)
            .await
            .map_err(|_| AppError::NotFound("path not found".into()))?
    };

    let dir_data = read_fs_dir_data(repos, repo_id, &parent_fs_id)
        .await
        .map_err(|_| AppError::NotFound("path not found".into()))?;

    let is_dir = dir_data
        .dirents
        .iter()
        .find(|d| d.name == entry_name)
        .map(|d| d.mode == infra::common::S_IFDIR)
        .unwrap_or(false);

    Ok(if is_dir {
        "d".to_string()
    } else {
        "f".to_string()
    })
}

// ── Response types ────────────────────────────────────────────────────

#[derive(Serialize)]
pub struct ShareLinkInfo {
    pub token: String,
    pub link: String,
    pub repo_id: String,
    pub path: String,
    pub created_at: i64,
    pub has_password: bool,
    pub expire_at: Option<i64>,
    pub s_type: String,
    pub view_cnt: i64,
    pub description: Option<String>,
}

#[derive(Serialize)]
pub struct ShareMember {
    pub email: String,
    pub permission: String,
    pub created_at: i64,
}

/// Result returned by `beshare_repo`.
pub struct BeshareResult {
    pub already_shared: bool,
}

// ── Share link operations (v2) ────────────────────────────────────────

pub async fn list_share_links(
    repos: &Repositories,
    user_id: i32,
) -> Result<Vec<ShareLinkInfo>, AppError> {
    let links = repos.share_link.find_by_creator_id(user_id).await?;

    let infos: Vec<ShareLinkInfo> = links
        .into_iter()
        .map(|l| ShareLinkInfo {
            token: l.token.clone(),
            link: format!("/f/{}/", l.token),
            repo_id: l.repo_id,
            path: l.path,
            created_at: l.created_at,
            has_password: l.password.is_some(),
            expire_at: l.expires_at,
            s_type: l.s_type,
            view_cnt: l.view_cnt,
            description: l.description,
        })
        .collect();

    Ok(infos)
}

pub async fn list_share_links_for_path(
    repos: &Repositories,
    repo_id: &str,
    path: &str,
) -> Result<Vec<ShareLinkInfo>, AppError> {
    let links = repos
        .share_link
        .find_by_repo_and_path(repo_id, path)
        .await?;

    let infos: Vec<ShareLinkInfo> = links
        .into_iter()
        .map(|l| ShareLinkInfo {
            token: l.token.clone(),
            link: format!("/f/{}/", l.token),
            repo_id: l.repo_id,
            path: l.path,
            created_at: l.created_at,
            has_password: l.password.is_some(),
            expire_at: l.expires_at,
            s_type: l.s_type,
            view_cnt: l.view_cnt,
            description: l.description,
        })
        .collect();

    Ok(infos)
}

pub async fn create_share_link(
    _db: &DatabaseConnection,
    repos: &Repositories,
    config: &Config,
    repo_id: &str,
    path: &str,
    password: Option<&str>,
    expires_at: Option<i64>,
    creator_id: i32,
) -> Result<ShareLinkInfo, AppError> {
    // Block share links for encrypted repos
    let repo_model = repos
        .repo
        .find_by_id(repo_id)
        .await?
        .ok_or_else(|| AppError::NotFound("repo not found".into()))?;
    if repo_model.encrypted != 0 {
        return Err(AppError::BadRequest(
            "cannot create share link for encrypted library".into(),
        ));
    }

    // Verify caller has read permission on the repo
    crate::domain::permission::check_repo_read_permission(
        repos.member.as_ref(),
        repo_id,
        creator_id,
    )
    .await?;

    let s_type = resolve_entry_type_raw(repos, repo_id, path).await?;

    let token = generate_share_link_token();
    let now = chrono::Utc::now().timestamp();

    let password_hash = password.map(|p| hash_password(p, config.auth.password_hash_iterations));

    repos
        .share_link
        .create_share_link(crate::repository::share_link::CreateShareLinkParams {
            repo_id: repo_id.to_string(),
            creator_id,
            path: path.to_string(),
            token: token.clone(),
            password: password_hash,
            expires_at,
            created_at: now,
            s_type: s_type.clone(),
            description: None,
        })
        .await?;

    let link = if s_type == "d" {
        format!("/d/{}/", token)
    } else {
        format!("/f/{}/", token)
    };

    Ok(ShareLinkInfo {
        token: token.clone(),
        link,
        repo_id: repo_id.to_string(),
        path: path.to_string(),
        created_at: now,
        has_password: password.is_some(),
        expire_at: expires_at,
        s_type,
        view_cnt: 0,
        description: None,
    })
}

pub async fn delete_share_link(
    repos: &Repositories,
    token: &str,
    user_id: i32,
) -> Result<(), AppError> {
    repos
        .share_link
        .delete_by_token_and_user(token, user_id)
        .await?;
    Ok(())
}

// ── Share link operations (v21) ───────────────────────────────────────

pub struct CreateShareLinkResult {
    pub token: String,
    pub s_type: String,
}

pub async fn create_share_link_v21(
    _db: &DatabaseConnection,
    repos: &Repositories,
    config: &Config,
    repo_id: &str,
    path: &str,
    password: Option<&str>,
    expire_days: Option<i64>,
    description: Option<&str>,
    creator_id: i32,
) -> Result<CreateShareLinkResult, AppError> {
    // Block share links for encrypted repos
    let repo_model = repos
        .repo
        .find_by_id(repo_id)
        .await?
        .ok_or_else(|| AppError::NotFound("repo not found".into()))?;
    if repo_model.encrypted != 0 {
        return Err(AppError::BadRequest(
            "cannot create share link for encrypted library".into(),
        ));
    }

    // Verify caller has read permission on the repo
    crate::domain::permission::check_repo_read_permission(
        repos.member.as_ref(),
        repo_id,
        creator_id,
    )
    .await?;

    let s_type = resolve_entry_type_raw(repos, repo_id, path).await?;

    let token = generate_share_link_token();
    let now = chrono::Utc::now().timestamp();

    repos
        .share_link
        .create_share_link(crate::repository::share_link::CreateShareLinkParams {
            repo_id: repo_id.to_string(),
            creator_id,
            path: path.to_string(),
            token: token.clone(),
            password: password.map(|p| hash_password(p, config.auth.password_hash_iterations)),
            expires_at: expire_days.map(|d| now + d * 86400),
            created_at: now,
            s_type: s_type.clone(),
            description: description.map(|s| s.to_string()),
        })
        .await?;

    Ok(CreateShareLinkResult { token, s_type })
}

pub async fn delete_share_link_v21(
    repos: &Repositories,
    token: &str,
    user_id: i32,
) -> Result<bool, AppError> {
    let result = repos
        .share_link
        .delete_by_token_and_user(token, user_id)
        .await?;
    Ok(result.rows_affected > 0)
}

pub async fn update_share_link_v21(
    db: &DatabaseConnection,
    config: &Config,
    repos: &Repositories,
    token: &str,
    user_id: i32,
    password: Option<Option<String>>,
    expire_days: Option<Option<i64>>,
    description: Option<Option<String>>,
) -> Result<ShareLinkInfo, AppError> {
    let now = chrono::Utc::now().timestamp();

    // Find and validate ownership
    let link = repos
        .share_link
        .find_by_token(token)
        .await?
        .ok_or_else(|| AppError::NotFound("Share link not found".into()))?;
    if link.creator_id != user_id {
        return Err(AppError::NotFound("Share link not found".into()));
    }

    // Build conditional update (only set fields that were provided)
    let mut active = share_link::ActiveModel {
        ..Default::default()
    };
    let new_password =
        password.map(|pwd| pwd.map(|p| hash_password(&p, config.auth.password_hash_iterations)));
    if let Some(ref pwd) = new_password {
        active.password = Set(pwd.clone());
    }
    let new_expire_at = expire_days.map(|days| days.map(|d| now + d * 86400));
    if let Some(ref val) = new_expire_at {
        active.expires_at = Set(*val);
    }
    let new_description = description.clone();
    if let Some(val) = new_description {
        active.description = Set(val);
    }

    share_link::Entity::update_many()
        .filter(share_link::Column::Id.eq(link.id))
        .set(active)
        .exec(db)
        .await?;

    // Compute effective values for the response using original + requested changes
    let effective_password = new_password.flatten().or(link.password);
    let effective_expire_at = new_expire_at.flatten().or(link.expires_at);
    let effective_description = description.flatten().or(link.description);

    let link_url = if link.s_type == "d" {
        format!("/d/{}/", link.token)
    } else {
        format!("/f/{}/", link.token)
    };

    Ok(ShareLinkInfo {
        token: link.token,
        link: link_url,
        repo_id: link.repo_id,
        path: link.path,
        created_at: link.created_at,
        has_password: effective_password.is_some(),
        expire_at: effective_expire_at,
        s_type: link.s_type,
        view_cnt: link.view_cnt,
        description: effective_description,
    })
}

// ── Repo sharing operations ──────────────────────────────────────────

/// Share (beshare) a repo with another user.
pub async fn beshare_repo(
    _db: &DatabaseConnection,
    repos: &Repositories,
    notification_manager: Option<&crate::notification::manager::NotificationManager>,
    repo_id: &str,
    caller_user_id: i32,
    user_email: &str,
    permission: Option<&str>,
) -> Result<BeshareResult, AppError> {
    if user_email.is_empty() {
        return Err(AppError::BadRequest("user email is required".into()));
    }

    // Verify caller has write permission on the repo
    crate::domain::permission::check_repo_write_permission(
        repos.member.as_ref(),
        repo_id,
        caller_user_id,
    )
    .await?;

    // Find the target user
    let target_user = repos
        .user
        .find_by_email(user_email)
        .await?
        .ok_or_else(|| AppError::BadRequest("user not found".into()))?;

    // Check if the membership already exists
    let existing = repos
        .member
        .find_by_repo_and_user(repo_id, target_user.id)
        .await?;

    if existing.is_some() {
        return Ok(BeshareResult {
            already_shared: true,
        });
    }

    // Add repo member
    let now = chrono::Utc::now().timestamp();
    let perm = permission.unwrap_or("rw").to_string();

    repos
        .member
        .create_member(crate::repository::member::CreateMemberParams {
            repo_id: repo_id.to_string(),
            user_id: target_user.id,
            permission: perm.clone(),
            created_at: now,
        })
        .await?;

    // Send WebSocket notification about the share change.
    if let Some(mgr) = notification_manager {
        let event = FolderPermEvent {
            repo_id: repo_id.to_string(),
            path: "/".to_string(),
            event_type: "user".to_string(),
            change_event: "add".to_string(),
            user: user_email.to_string(),
            group: -1,
            perm,
        };
        mgr.notify(event).await;
    }

    Ok(BeshareResult {
        already_shared: false,
    })
}

/// List all share members for a repo.
pub async fn list_share_members(
    repos: &Repositories,
    repo_id: &str,
) -> Result<Vec<ShareMember>, AppError> {
    let members = repos.member.find_by_repo_id(repo_id).await?;

    let mut result = Vec::new();
    for m in members {
        let user_record = repos.user.find_by_id(m.user_id).await?;
        if let Some(u) = user_record {
            result.push(ShareMember {
                email: u.email,
                permission: m.permission,
                created_at: m.created_at,
            });
        }
    }
    Ok(result)
}

/// Modify a user's share permission on a repo.
pub async fn modify_share_permission(
    _db: &DatabaseConnection,
    repos: &Repositories,
    notification_manager: Option<&crate::notification::manager::NotificationManager>,
    repo_id: &str,
    caller_user_id: i32,
    user_email: &str,
    new_permission: &str,
) -> Result<(), AppError> {
    if user_email.is_empty() {
        return Err(AppError::BadRequest("user email is required".into()));
    }
    if new_permission != "rw" && new_permission != "r" {
        return Err(AppError::BadRequest(
            "permission must be 'rw' or 'r'".into(),
        ));
    }

    // Only the repo owner can modify permissions.
    crate::domain::permission::check_repo_write_permission(
        repos.member.as_ref(),
        repo_id,
        caller_user_id,
    )
    .await?;

    let target_user = repos
        .user
        .find_by_email(user_email)
        .await?
        .ok_or_else(|| AppError::BadRequest("user not found".into()))?;

    // Verify the target user is a member of this repo.
    let _member = repos
        .member
        .find_by_repo_and_user(repo_id, target_user.id)
        .await?
        .ok_or_else(|| AppError::BadRequest("user is not a member of this repo".into()))?;

    repos
        .member
        .update_permission(repo_id, target_user.id, new_permission)
        .await?;

    // Send WebSocket notification about the permission change.
    if let Some(mgr) = notification_manager {
        let event = FolderPermEvent {
            repo_id: repo_id.to_string(),
            path: "/".to_string(),
            event_type: "user".to_string(),
            change_event: "modify".to_string(),
            user: user_email.to_string(),
            group: -1,
            perm: new_permission.to_string(),
        };
        mgr.notify(event).await;
    }

    Ok(())
}

/// Remove a user's share from a repo.
pub async fn delete_share(
    _db: &DatabaseConnection,
    repos: &Repositories,
    notification_manager: Option<&crate::notification::manager::NotificationManager>,
    repo_id: &str,
    caller_user_id: i32,
    user_email: &str,
) -> Result<(), AppError> {
    if user_email.is_empty() {
        return Err(AppError::BadRequest("user email is required".into()));
    }

    // Only the repo owner can delete shares.
    crate::domain::permission::check_repo_write_permission(
        repos.member.as_ref(),
        repo_id,
        caller_user_id,
    )
    .await?;

    let target_user = repos
        .user
        .find_by_email(user_email)
        .await?
        .ok_or_else(|| AppError::BadRequest("user not found".into()))?;

    repos
        .member
        .delete_by_repo_and_user(repo_id, target_user.id)
        .await?;

    // Send WebSocket notification about the share deletion.
    if let Some(mgr) = notification_manager {
        let event = FolderPermEvent {
            repo_id: repo_id.to_string(),
            path: "/".to_string(),
            event_type: "user".to_string(),
            change_event: "del".to_string(),
            user: user_email.to_string(),
            group: -1,
            perm: String::new(),
        };
        mgr.notify(event).await;
    }

    Ok(())
}

/// Look up a share link, check expiry, return the link model or error.
pub async fn resolve_share_link(
    repos: &Repositories,
    token: &str,
) -> Result<infra::entity::share_link::Model, AppError> {
    let link = repos
        .share_link
        .find_by_token(token)
        .await?
        .ok_or_else(|| AppError::NotFound("Link not found".into()))?;

    if let Some(expires_at) = link.expires_at
        && chrono::Utc::now().timestamp() > expires_at
    {
        return Err(AppError::NotFound("Link has expired".into()));
    }

    Ok(link)
}

/// Check whether the password in the request matches the stored hash.
pub fn check_share_link_password(
    link: &infra::entity::share_link::Model,
    provided_password: Option<&str>,
    password_hash_iterations: u32,
) -> Result<bool, AppError> {
    let stored_hash = match &link.password {
        Some(h) => h,
        None => return Ok(true),
    };

    match provided_password {
        Some(pwd) => Ok(crate::service::auth::password::verify_password(
            pwd,
            stored_hash,
            password_hash_iterations,
        )),
        None => Ok(false),
    }
}

/// Fire-and-forget view count increment.
pub fn increment_view_cnt(
    share_link_repo: Arc<dyn crate::repository::ShareLinkRepository>,
    link_id: i32,
) {
    tokio::spawn(async move {
        let _ = share_link_repo.increment_view_cnt(link_id).await;
    });
}
