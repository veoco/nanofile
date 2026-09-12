use serde::Serialize;
use std::sync::Arc;

use crate::Config;
use crate::fs::core::tree::{read_fs_dir_data, resolve_fs_id};
use crate::notification::events::FolderPermEvent;
use crate::repository::Repositories;
use crate::service::auth::password::hash_password;
use crate::service::auth::token::generate_share_link_token;
use base::error::AppError;
use infra::entity::{share_link, user};

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

/// Build an absolute share-link URL, matching seahub's `gen_shared_link()`
/// (`{service_url}/f/{token}/` or `{service_url}/d/{token}/`). Clients
/// (notably the Android app) share/copy the `link` field verbatim.
fn share_link_url(s_type: &str, token: &str, base_url: &str) -> String {
    let base = base_url.trim_end_matches('/');
    if s_type == "d" {
        format!("{base}/d/{token}/")
    } else {
        format!("{base}/f/{token}/")
    }
}

/// Map a share-link model to its API response shape.
fn share_link_info_from_model(l: &share_link::Model, base_url: &str) -> ShareLinkInfo {
    ShareLinkInfo {
        token: l.token.clone(),
        link: share_link_url(&l.s_type, &l.token, base_url),
        repo_id: l.repo_id.clone(),
        path: l.path.clone(),
        created_at: l.created_at,
        has_password: l.password.is_some(),
        expire_at: l.expires_at,
        s_type: l.s_type.clone(),
        view_cnt: l.view_cnt,
        description: l.description.clone(),
    }
}

/// Result returned by `beshare_repo`.
pub struct BeshareResult {
    pub already_shared: bool,
}

// ── Share link operations (v2) ────────────────────────────────────────

pub async fn list_share_links(
    repos: &Repositories,
    base_url: &str,
    user_id: i32,
) -> Result<Vec<ShareLinkInfo>, AppError> {
    let links = repos.share_link.find_by_creator_id(user_id).await?;

    let infos: Vec<ShareLinkInfo> = links
        .iter()
        .map(|l| share_link_info_from_model(l, base_url))
        .collect();

    Ok(infos)
}

pub async fn list_share_links_for_path(
    repos: &Repositories,
    base_url: &str,
    repo_id: &str,
    path: &str,
    creator_id: i32,
) -> Result<Vec<ShareLinkInfo>, AppError> {
    let links = repos
        .share_link
        .find_by_repo_and_path(repo_id, path)
        .await?;

    // Only the caller's own links: share tokens are unauthenticated bearer
    // URLs, so a read-only member must not learn other members' tokens.
    let infos: Vec<ShareLinkInfo> = links
        .into_iter()
        .filter(|l| l.creator_id == creator_id)
        .map(|l| share_link_info_from_model(&l, base_url))
        .collect();

    Ok(infos)
}

/// Shared implementation for creating a share link (used by v2, v2.1 and the
/// web UI form handler). Routing every creation path through this one function
/// is what keeps the encrypted-library block, the membership check and the
/// path validation from being bypassed by a caller that forgets them.
pub(crate) async fn create_share_link_impl(
    repos: &Repositories,
    config: &Config,
    repo_id: &str,
    path: &str,
    password: Option<&str>,
    expires_at: Option<i64>,
    description: Option<String>,
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
            description: description.clone(),
        })
        .await?;

    let link = share_link_url(&s_type, &token, &config.server.site_url);

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
        description,
    })
}

pub async fn create_share_link(
    repos: &Repositories,
    config: &Config,
    repo_id: &str,
    path: &str,
    password: Option<&str>,
    expires_at: Option<i64>,
    creator_id: i32,
) -> Result<ShareLinkInfo, AppError> {
    create_share_link_impl(
        repos, config, repo_id, path, password, expires_at, None, creator_id,
    )
    .await
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

pub async fn create_share_link_v21(
    repos: &Repositories,
    config: &Config,
    repo_id: &str,
    path: &str,
    password: Option<&str>,
    expire_days: Option<i64>,
    description: Option<&str>,
    creator_id: i32,
) -> Result<ShareLinkInfo, AppError> {
    let now = chrono::Utc::now().timestamp();
    let expires_at = expire_days.map(|d| now + d * 86400);
    create_share_link_impl(
        repos,
        config,
        repo_id,
        path,
        password,
        expires_at,
        description.map(|s| s.to_string()),
        creator_id,
    )
    .await
}

/// GET /api/v2.1/share-links/{token}/ — retrieve share link details
pub async fn get_share_link_v21(
    repos: &Repositories,
    base_url: &str,
    token: &str,
    user_id: i32,
) -> Result<ShareLinkInfo, AppError> {
    let link = repos
        .share_link
        .find_by_token(token)
        .await?
        .ok_or_else(|| AppError::NotFound("Share link not found".into()))?;
    if link.creator_id != user_id {
        return Err(AppError::NotFound("Share link not found".into()));
    }

    Ok(share_link_info_from_model(&link, base_url))
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

    // Prepare field updates (business logic: password hashing, expire_days conversion)
    let new_password =
        password.map(|pwd| pwd.map(|p| hash_password(&p, config.auth.password_hash_iterations)));
    let new_expire_at = expire_days.map(|days| days.map(|d| now + d * 86400));
    let new_description = description.clone();

    repos
        .share_link
        .update_share_link_fields(
            link.id,
            new_password.clone(),
            new_expire_at,
            new_description,
        )
        .await?;

    // Compute effective values for the response using original + requested changes
    let effective_password = new_password.flatten().or(link.password);
    let effective_expire_at = new_expire_at.flatten().or(link.expires_at);
    let effective_description = description.flatten().or(link.description);

    let link_url = share_link_url(&link.s_type, &link.token, &config.server.site_url);

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

    crate::domain::permission::check_repo_owner(repos.member.as_ref(), repo_id, caller_user_id)
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
    // Validate the level like `modify_share_permission` does. Write access
    // requires exactly "rw" but *read* is granted for any non-NULL value, so an
    // unvalidated "" / "none" / typo silently produced a read-share instead of
    // being rejected.
    let perm = permission.unwrap_or("rw").to_string();
    if perm != "rw" && perm != "r" {
        return Err(AppError::BadRequest(
            "permission must be 'rw' or 'r'".into(),
        ));
    }

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

    // Batch-load all member users in one query instead of one per member.
    let user_ids: Vec<i32> = members.iter().map(|m| m.user_id).collect();
    let users: std::collections::HashMap<i32, user::Model> = repos
        .user
        .find_by_ids(&user_ids)
        .await?
        .into_iter()
        .map(|u| (u.id, u))
        .collect();

    let mut result = Vec::new();
    for m in members {
        if let Some(u) = users.get(&m.user_id) {
            result.push(ShareMember {
                email: u.email.clone(),
                permission: m.permission,
                created_at: m.created_at,
            });
        }
    }
    Ok(result)
}

/// Modify a user's share permission on a repo.
pub async fn modify_share_permission(
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

    crate::domain::permission::check_repo_owner(repos.member.as_ref(), repo_id, caller_user_id)
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
///
/// `password_manager`, when supplied, also drops the removed member's cached
/// library key. Every read path re-checks membership before using that cache, so
/// this is defence in depth rather than the primary control.
pub async fn delete_share(
    repos: &Repositories,
    notification_manager: Option<&crate::notification::manager::NotificationManager>,
    password_manager: Option<&infra::crypto::password_manager::PasswordManager>,
    repo_id: &str,
    caller_user_id: i32,
    user_email: &str,
) -> Result<(), AppError> {
    if user_email.is_empty() {
        return Err(AppError::BadRequest("user email is required".into()));
    }

    crate::domain::permission::check_repo_owner(repos.member.as_ref(), repo_id, caller_user_id)
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

    // The member's share/upload links must stop resolving at once, not after the
    // creator-access cache expires.
    invalidate_link_creator_cache(target_user.id, Some(repo_id));

    // A sync token is bound to (repo, user) for up to `sync_token_ttl_days`
    // (a year by default) and the `/seafhttp/` endpoints re-derive the caller's
    // identity from it, so the removed member would otherwise keep a
    // block-existence oracle (and the locked-file list) for the library long
    // after losing access.
    if let Err(e) = repos
        .sync_token
        .delete_by_repo_and_user(repo_id, target_user.id)
        .await
    {
        tracing::warn!(
            "failed to revoke sync tokens for user {} on repo {repo_id}: {e}",
            target_user.id
        );
    }

    // Likewise for the decrypted library key cached for the removed member.
    if let Some(pm) = password_manager {
        pm.remove_password(repo_id, target_user.id).await;
    }

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

/// How long the "may the creator still act on this library" answer is cached.
///
/// This is checked on every anonymous link request, and the answer only changes
/// when an administrator revokes access — so a short TTL keeps the common path
/// free of two extra queries while bounding the window in which a revoked
/// member's links still resolve.
const LINK_CREATOR_CACHE_TTL: std::time::Duration = std::time::Duration::from_secs(30);

/// Hard cap on the cache, so a flood of distinct links cannot grow it forever.
const LINK_CREATOR_CACHE_MAX: usize = 10_000;

type LinkCreatorCache =
    std::sync::Mutex<std::collections::HashMap<(i32, String, bool), (bool, std::time::Instant)>>;

static LINK_CREATOR_CACHE: std::sync::LazyLock<LinkCreatorCache> =
    std::sync::LazyLock::new(|| std::sync::Mutex::new(std::collections::HashMap::new()));

/// Whether the user who created a link may still act on `repo_id`.
///
/// A share/upload link is a capability handed to third parties, but it acts
/// **as its creator**: it exposes library content (share link) or accepts new
/// content (upload link) in a library the creator may since have been removed
/// from, or whose account may have been deactivated. Resolving a link therefore
/// re-checks the creator rather than trusting the link alone — the same rule the
/// download and upload tokens already apply, where "the token outlives
/// membership".
///
/// Results are cached for [`LINK_CREATOR_CACHE_TTL`] because this runs on every
/// anonymous request; the revocation window is therefore at most that long.
pub async fn link_creator_may_access(
    repos: &Repositories,
    creator_id: i32,
    repo_id: &str,
    need_write: bool,
) -> bool {
    let key = (creator_id, repo_id.to_string(), need_write);
    {
        let cache = LINK_CREATOR_CACHE.lock().unwrap_or_else(|e| e.into_inner());
        if let Some((allowed, checked_at)) = cache.get(&key)
            && checked_at.elapsed() < LINK_CREATOR_CACHE_TTL
        {
            return *allowed;
        }
    }

    let allowed = match repos.user.find_by_id(creator_id).await {
        Ok(Some(user)) if user.is_active => {
            let member = repos.member.as_ref();
            if need_write {
                crate::domain::permission::check_repo_write_permission(member, repo_id, creator_id)
                    .await
                    .is_ok()
            } else {
                crate::domain::permission::check_repo_read_permission(member, repo_id, creator_id)
                    .await
                    .is_ok()
            }
        }
        // Deactivated or deleted creator: the link stops working.
        _ => false,
    };

    let mut cache = LINK_CREATOR_CACHE.lock().unwrap_or_else(|e| e.into_inner());
    if cache.len() >= LINK_CREATOR_CACHE_MAX {
        cache.retain(|_, (_, checked_at)| checked_at.elapsed() < LINK_CREATOR_CACHE_TTL);
        if cache.len() >= LINK_CREATOR_CACHE_MAX {
            cache.clear();
        }
    }
    cache.insert(key, (allowed, std::time::Instant::now()));
    allowed
}

/// Drop cached "creator may access" decisions.
///
/// Called whenever membership or account state changes so that revoking access
/// takes effect immediately instead of after [`LINK_CREATOR_CACHE_TTL`]. Passing
/// `repo_id = None` drops every entry for that user (account-level changes such
/// as deactivation).
pub fn invalidate_link_creator_cache(user_id: i32, repo_id: Option<&str>) {
    let mut cache = LINK_CREATOR_CACHE.lock().unwrap_or_else(|e| e.into_inner());
    cache.retain(|(creator_id, cached_repo, _), _| {
        if *creator_id != user_id {
            return true;
        }
        match repo_id {
            Some(repo) => cached_repo != repo,
            None => false,
        }
    });
}

/// Look up a share link, check expiry and that its creator still has access,
/// return the link model or error.
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

    // The link acts as its creator, so it must stop resolving once the creator
    // loses access (member removed, account deactivated). "Not found" keeps the
    // response indistinguishable from an unknown token.
    if !link_creator_may_access(repos, link.creator_id, &link.repo_id, false).await {
        return Err(AppError::NotFound("Link not found".into()));
    }

    Ok(link)
}

/// Check whether the password in the request matches the stored hash.
pub async fn check_share_link_password(
    link: &infra::entity::share_link::Model,
    provided_password: Option<&str>,
    password_hash_iterations: u32,
) -> Result<bool, AppError> {
    let stored_hash = match &link.password {
        Some(h) => h,
        None => return Ok(true),
    };

    match provided_password {
        Some(pwd) => Ok(crate::service::auth::password::verify_password_async(
            pwd.to_string(),
            stored_hash.clone(),
            password_hash_iterations,
        )
        .await),
        None => Ok(false),
    }
}

/// Fire-and-forget view count increment.
pub fn increment_view_cnt(
    share_link_repo: Arc<dyn crate::repository::share_link::ShareLinkRepository>,
    link_id: i32,
) {
    tokio::spawn(async move {
        let _ = share_link_repo.increment_view_cnt(link_id).await;
    });
}
