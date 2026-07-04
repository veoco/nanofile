use sea_orm::{ActiveModelTrait, ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter, Set};
use serde::Serialize;

use crate::Config;
use crate::auth::password::hash_password;
use crate::auth::token::generate_share_link_token;
use crate::entity::{repo_member, share_link};
use crate::error::AppError;
use crate::notification::events::FolderPermEvent;
use crate::repository::Repositories;
use crate::storage;

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

pub async fn create_share_link(
    db: &DatabaseConnection,
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
    storage::check_repo_read_permission(db, repo_id, creator_id).await?;

    // s_type defaults to 'f' (file). Full path-to-type resolution requires
    // walking the commit tree, which is done lazily at download time.
    let s_type = "f".to_string();

    let token = generate_share_link_token();
    let now = chrono::Utc::now().timestamp();

    let password_hash = password.map(|p| hash_password(p, config.auth.password_hash_iterations));

    let model = share_link::ActiveModel {
        id: sea_orm::NotSet,
        repo_id: Set(repo_id.to_string()),
        creator_id: Set(creator_id),
        path: Set(path.to_string()),
        token: Set(token.clone()),
        password: Set(password_hash),
        expires_at: Set(expires_at),
        created_at: Set(now),
        s_type: Set(s_type.clone()),
        view_cnt: Set(0i64),
        description: Set(None),
    };
    share_link::Entity::insert(model).exec(db).await?;

    Ok(ShareLinkInfo {
        token: token.clone(),
        link: format!("/f/{}/", token),
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
    db: &DatabaseConnection,
    token: &str,
    user_id: i32,
) -> Result<(), AppError> {
    share_link::Entity::delete_many()
        .filter(share_link::Column::Token.eq(token))
        .filter(share_link::Column::CreatorId.eq(user_id))
        .exec(db)
        .await?;

    Ok(())
}

// ── Share link operations (v21) ───────────────────────────────────────

pub async fn create_share_link_v21(
    db: &DatabaseConnection,
    repos: &Repositories,
    config: &Config,
    repo_id: &str,
    path: &str,
    password: Option<&str>,
    expire_days: Option<i64>,
    creator_id: i32,
) -> Result<String, AppError> {
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
    storage::check_repo_read_permission(db, repo_id, creator_id).await?;

    let s_type = "f".to_string();

    let token = generate_share_link_token();
    let now = chrono::Utc::now().timestamp();

    share_link::Entity::insert(share_link::ActiveModel {
        id: sea_orm::NotSet,
        repo_id: Set(repo_id.to_string()),
        creator_id: Set(creator_id),
        path: Set(path.to_string()),
        token: Set(token.clone()),
        password: Set(password.map(|p| hash_password(p, config.auth.password_hash_iterations))),
        expires_at: Set(expire_days.map(|d| now + d * 86400)),
        created_at: Set(now),
        s_type: Set(s_type),
        view_cnt: Set(0i64),
        description: Set(None),
    })
    .exec(db)
    .await?;

    Ok(token)
}

pub async fn delete_share_link_v21(
    db: &DatabaseConnection,
    token: &str,
    user_id: i32,
) -> Result<bool, AppError> {
    let result = share_link::Entity::delete_many()
        .filter(share_link::Column::Token.eq(token))
        .filter(share_link::Column::CreatorId.eq(user_id))
        .exec(db)
        .await?;
    Ok(result.rows_affected > 0)
}

pub async fn update_share_link_v21(
    db: &DatabaseConnection,
    _repos: &Repositories,
    token: &str,
    user_id: i32,
    expire_days: Option<Option<i64>>,
    description: Option<Option<String>>,
) -> Result<ShareLinkInfo, AppError> {
    let now = chrono::Utc::now().timestamp();

    // Find and validate ownership
    let link = share_link::Entity::find()
        .filter(share_link::Column::Token.eq(token))
        .filter(share_link::Column::CreatorId.eq(user_id))
        .one(db)
        .await?
        .ok_or_else(|| AppError::NotFound("Share link not found".into()))?;

    let mut active: share_link::ActiveModel = link.into();

    if let Some(days) = expire_days {
        active.expires_at = Set(days.map(|d| now + d * 86400));
    }
    if let Some(desc) = description {
        active.description = Set(desc);
    }

    let updated = active.update(db).await?;
    let token = updated.token.clone();

    Ok(ShareLinkInfo {
        token,
        link: format!("/f/{}/", updated.token),
        repo_id: updated.repo_id,
        path: updated.path,
        created_at: updated.created_at,
        has_password: updated.password.is_some(),
        expire_at: updated.expires_at,
        s_type: updated.s_type,
        view_cnt: updated.view_cnt,
        description: updated.description,
    })
}

// ── Repo sharing operations ──────────────────────────────────────────

/// Share (beshare) a repo with another user.
pub async fn beshare_repo(
    db: &DatabaseConnection,
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
    storage::check_repo_write_permission(db, repo_id, caller_user_id).await?;

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

    repo_member::Entity::insert(repo_member::ActiveModel {
        id: sea_orm::NotSet,
        repo_id: Set(repo_id.to_string()),
        user_id: Set(target_user.id),
        permission: Set(perm.clone()),
        created_at: Set(now),
    })
    .exec(db)
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
    db: &DatabaseConnection,
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
    storage::check_repo_write_permission(db, repo_id, caller_user_id).await?;

    let target_user = repos
        .user
        .find_by_email(user_email)
        .await?
        .ok_or_else(|| AppError::BadRequest("user not found".into()))?;

    let member = repos
        .member
        .find_by_repo_and_user(repo_id, target_user.id)
        .await?
        .ok_or_else(|| AppError::BadRequest("user is not a member of this repo".into()))?;

    let mut active: repo_member::ActiveModel = member.into();
    active.permission = Set(new_permission.to_string());
    active.update(db).await?;

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
    db: &DatabaseConnection,
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
    storage::check_repo_write_permission(db, repo_id, caller_user_id).await?;

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
