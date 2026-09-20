use serde::Serialize;
use std::sync::Arc;

use crate::Config;
use crate::fs::core::tree::{read_fs_dir_data, resolve_fs_id};
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

/// How long the "may the creator still act on this library" answer is cached.
///
/// This is checked on every anonymous link request, and the answer only changes
/// when an administrator revokes access or the library changes hands — so a
/// short TTL keeps the common path free of two extra queries while bounding the
/// window in which a revoked account's links still resolve.
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
/// content (upload link) in a library the creator may since have lost, or whose
/// account may have been deactivated. Resolving a link therefore re-checks the
/// creator rather than trusting the link alone — the same rule the download and
/// upload tokens already apply, where "the token outlives the grant".
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

/// The wire-protocol message for a link that does not resolve. Exposed so the
/// HTML page can tell "turned off" apart from "expired" without matching a
/// literal it does not own.
pub const LINK_NOT_FOUND: &str = "Link not found";

/// The wire-protocol message for a link that ran out of time.
pub const LINK_EXPIRED: &str = "Link has expired";

/// The same message for upload links, which have their own lookup.
pub const UPLOAD_LINK_NOT_FOUND: &str = "Upload link not found";

/// The same message for upload links that ran out of time.
pub const UPLOAD_LINK_EXPIRED: &str = "Upload link has expired";

/// Why a link stopped resolving.
///
/// The wire protocol flattens both into one 404 — every Seafile client only
/// distinguishes "resolves" from "does not" — but an HTML page should not tell
/// the person holding an expired link the same thing as one holding a link the
/// owner switched off.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinkFailure {
    /// Unknown token, revoked link, deleted library, or a creator who lost
    /// access. Kept indistinguishable on purpose: the wire response must not
    /// confirm that a token exists.
    Unknown,
    Expired,
}

/// Classify the result of a share- or upload-link lookup for the HTML pages.
///
/// Only a lookup's own `NotFound` is classified: for those calls it always
/// means "this link does not resolve", so a database or IO failure stays a 500
/// rather than being dressed up as a dead link. `None` means "not a link
/// failure — propagate it unchanged".
pub fn classify_link_failure(err: &AppError) -> Option<LinkFailure> {
    match err {
        AppError::NotFound(msg) if msg == LINK_EXPIRED || msg == UPLOAD_LINK_EXPIRED => {
            Some(LinkFailure::Expired)
        }
        AppError::NotFound(_) => Some(LinkFailure::Unknown),
        _ => None,
    }
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
        .ok_or_else(|| AppError::NotFound(LINK_NOT_FOUND.into()))?;

    if let Some(expires_at) = link.expires_at
        && chrono::Utc::now().timestamp() > expires_at
    {
        return Err(AppError::NotFound(LINK_EXPIRED.into()));
    }

    // The link acts as its creator, so it must stop resolving once the creator
    // loses access (member removed, account deactivated). "Not found" keeps the
    // response indistinguishable from an unknown token.
    if !link_creator_may_access(repos, link.creator_id, &link.repo_id, false).await {
        return Err(AppError::NotFound(LINK_NOT_FOUND.into()));
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

#[cfg(test)]
mod link_failure_tests {
    use super::*;

    #[test]
    fn a_link_failure_is_told_apart_from_a_real_error() {
        assert_eq!(
            classify_link_failure(&AppError::NotFound(LINK_EXPIRED.into())),
            Some(LinkFailure::Expired)
        );
        assert_eq!(
            classify_link_failure(&AppError::NotFound(UPLOAD_LINK_EXPIRED.into())),
            Some(LinkFailure::Expired)
        );
        // Every other NotFound from a lookup is the link itself being gone —
        // including the ones whose exact text is not spelled out here.
        assert_eq!(
            classify_link_failure(&AppError::NotFound("Repo not found".into())),
            Some(LinkFailure::Unknown)
        );
        // A lookup that failed for another reason must stay a 500.
        assert_eq!(
            classify_link_failure(&AppError::Internal("db down".into())),
            None
        );
        assert_eq!(classify_link_failure(&AppError::Forbidden), None);
    }
}
