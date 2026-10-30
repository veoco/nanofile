use serde::Serialize;
use std::sync::Arc;

use crate::Config;
use crate::domain::permission::RepoScope;
use crate::repository::Repositories;
use crate::service::auth::password::hash_password_async;
use crate::service::auth::token::{generate_share_link_token, generate_upload_link_token};
use base::error::AppError;
use infra::entity::upload_link;

#[derive(Serialize)]
pub struct UploadLinkInfo {
    pub token: String,
    pub link: String,
    pub repo_id: String,
    pub path: String,
    pub created_at: i64,
}

/// Absolute upload-link URL, matching seahub's `gen_shared_upload_link()` (`{service_url}/u/d/{token}/`).
fn upload_link_url(token: &str, base_url: &str) -> String {
    let base = base_url.trim_end_matches('/');
    format!("{base}/u/{token}/")
}

/// v2.1 list-item JSON for an upload link (no repo name / obj_name).
fn upload_link_json(l: &upload_link::Model) -> serde_json::Value {
    serde_json::json!({
        "token": l.token,
        "repo_id": l.repo_id,
        "path": l.path,
        "has_password": l.password.is_some(),
        "expire_at": l.expires_at,
        "view_cnt": l.view_cnt,
        "description": l.description,
    })
}

/// Object name for a path: `/` → `/`, otherwise the final path segment.
fn upload_link_obj_name(path: &str) -> String {
    if path == "/" {
        "/".to_string()
    } else {
        path.trim_end_matches('/')
            .rsplit_once('/')
            .map(|(_, n)| n.to_string())
            .unwrap_or_else(|| path.to_string())
    }
}

pub async fn list_upload_links(
    repos: &Repositories,
    base_url: &str,
    user_id: i32,
) -> Result<Vec<UploadLinkInfo>, AppError> {
    let links = repos.upload_link.find_by_creator_id(user_id).await?;

    let infos: Vec<UploadLinkInfo> = links
        .into_iter()
        .map(|l| UploadLinkInfo {
            token: l.token.clone(),
            link: upload_link_url(&l.token, base_url),
            repo_id: l.repo_id,
            path: l.path,
            created_at: l.created_at,
        })
        .collect();

    Ok(infos)
}

/// Shared implementation for creating an upload link (used by both v2 and v2.1).
async fn create_upload_link_impl(
    repos: &Repositories,
    config: &Config,
    repo_id: &str,
    path: &str,
    token: String,
    password: Option<&str>,
    expires_at: Option<i64>,
    description: Option<String>,
    creator_id: i32,
    scope: &RepoScope,
) -> Result<UploadLinkInfo, AppError> {
    if !scope.allows(repo_id) {
        return Err(AppError::Forbidden);
    }

    // An upload link grants anonymous *write* access to the repo, so only
    // members with write permission may create one (a read-only member must
    // not be able to mint a link that bypasses their read-only role).
    crate::domain::permission::check_repo_write_permission(
        repos.member.as_ref(),
        repo_id,
        creator_id,
    )
    .await?;

    // An encrypted library cannot be written through an anonymous link: the server stores
    // ciphertext, which needs the password-equivalent library key held in the per-user password
    // cache, and an anonymous visitor has no cache entry. The official clients already hide the
    // upload-link action for encrypted libraries (desktop `file-table.cpp:386`, Android
    // `BottomSheetMenuManager:365`), so this matches their expectation. Mirrors the share-link guard
    // in `sharing::share`.
    let repo_model = repos
        .repo
        .find_by_id(repo_id)
        .await?
        .ok_or_else(|| AppError::NotFound("repo not found".into()))?;
    if repo_model.encrypted != 0 {
        return Err(AppError::BadRequest(
            "cannot create upload link for encrypted library".into(),
        ));
    }

    let now = chrono::Utc::now().timestamp();
    let password_hash = match password {
        Some(p) => {
            Some(hash_password_async(p.to_string(), config.auth.password_hash_iterations).await)
        }
        None => None,
    };

    repos
        .upload_link
        .create_upload_link(crate::repository::upload_link::CreateUploadLinkParams {
            repo_id: repo_id.to_string(),
            creator_id,
            path: path.to_string(),
            token: token.clone(),
            password: password_hash,
            expires_at,
            created_at: now,
            description,
        })
        .await?;

    Ok(UploadLinkInfo {
        token: token.clone(),
        link: upload_link_url(&token, &config.server.site_url),
        repo_id: repo_id.to_string(),
        path: path.to_string(),
        created_at: now,
    })
}

pub async fn create_upload_link(
    repos: &Repositories,
    config: &Config,
    repo_id: &str,
    path: &str,
    password: Option<&str>,
    expires_at: Option<i64>,
    creator_id: i32,
    scope: &RepoScope,
) -> Result<UploadLinkInfo, AppError> {
    let token = generate_upload_link_token();
    create_upload_link_impl(
        repos, config, repo_id, path, token, password, expires_at, None, creator_id, scope,
    )
    .await
}

pub async fn delete_upload_link(
    repos: &Repositories,
    token: &str,
    user_id: i32,
    token_manager: Option<&Arc<crate::AccessTokenManager>>,
    scope: &RepoScope,
) -> Result<(), AppError> {
    // Resolve the row first: deleting the link must also revoke the upload tokens already minted from
    // it, otherwise someone who exchanged the link for a token can keep uploading for its remaining TTL.
    let link = repos.upload_link.find_by_token(token).await?;
    if let Some(link) = &link
        && (link.creator_id != user_id || !scope.allows(&link.repo_id))
    {
        return Err(AppError::NotFound("Upload link not found".into()));
    }
    repos
        .upload_link
        .delete_by_token_and_user(token, user_id)
        .await?;
    if let (Some(manager), Some(link)) = (token_manager, link) {
        let revoked = manager.revoke_upload_link(link.id);
        if revoked > 0 {
            tracing::debug!(link_id = link.id, revoked, "revoked upload-link tokens");
        }
    }
    Ok(())
}

pub async fn list_upload_links_v21(
    repos: &Repositories,
    user_id: i32,
) -> Result<Vec<serde_json::Value>, AppError> {
    let links = repos.upload_link.find_by_creator_id(user_id).await?;

    let items: Vec<serde_json::Value> = links.iter().map(upload_link_json).collect();

    Ok(items)
}

pub async fn list_upload_links_for_path(
    repos: &Repositories,
    repo_id: &str,
    path: &str,
    creator_id: i32,
) -> Result<Vec<serde_json::Value>, AppError> {
    let links = repos
        .upload_link
        .find_by_repo_and_path(repo_id, path)
        .await?;
    // Only the caller's own links: an upload token grants anonymous write, so
    // a read-only member must not learn other members' tokens.
    let items: Vec<serde_json::Value> = links
        .into_iter()
        .filter(|l| l.creator_id == creator_id)
        .map(|l| upload_link_json(&l))
        .collect();
    Ok(items)
}

pub async fn create_upload_link_v21(
    repos: &Repositories,
    config: &Config,
    repo_id: &str,
    path: &str,
    password: Option<String>,
    expire_days: Option<i64>,
    description: Option<String>,
    creator_id: i32,
    scope: &RepoScope,
) -> Result<UploadLinkInfo, AppError> {
    let token = generate_share_link_token();
    let now = chrono::Utc::now().timestamp();
    let expires_at = expire_days.map(|d| now + d * 86400);
    create_upload_link_impl(
        repos,
        config,
        repo_id,
        path,
        token,
        password.as_deref(),
        expires_at,
        description,
        creator_id,
        scope,
    )
    .await
}

pub async fn delete_upload_link_v21(
    repos: &Repositories,
    id: i32,
    user_id: i32,
    token_manager: Option<&Arc<crate::AccessTokenManager>>,
) -> Result<bool, AppError> {
    let result = repos.upload_link.delete_by_id_and_user(id, user_id).await?;
    let deleted = result.rows_affected > 0;
    if deleted && let Some(manager) = token_manager {
        let revoked = manager.revoke_upload_link(id);
        if revoked > 0 {
            tracing::debug!(link_id = id, revoked, "revoked upload-link tokens");
        }
    }
    Ok(deleted)
}

/// Delete an upload link by token string (seahub-compatible).
pub async fn delete_upload_link_v21_by_token(
    repos: &Repositories,
    token: &str,
    user_id: i32,
    token_manager: Option<&Arc<crate::AccessTokenManager>>,
    scope: &RepoScope,
) -> Result<bool, AppError> {
    let link = repos
        .upload_link
        .find_by_token(token)
        .await?
        .ok_or_else(|| AppError::NotFound("Upload link not found".into()))?;

    if link.creator_id != user_id {
        return Err(AppError::Forbidden);
    }
    if !scope.allows(&link.repo_id) {
        return Err(AppError::NotFound("Upload link not found".into()));
    }

    let result = repos
        .upload_link
        .delete_by_id_and_user(link.id, user_id)
        .await?;
    let deleted = result.rows_affected > 0;
    if deleted && let Some(manager) = token_manager {
        let revoked = manager.revoke_upload_link(link.id);
        if revoked > 0 {
            tracing::debug!(link_id = link.id, revoked, "revoked upload-link tokens");
        }
    }
    Ok(deleted)
}

pub async fn get_upload_link_v21(
    repos: &Repositories,
    base_url: &str,
    token: &str,
    user_id: i32,
    scope: &RepoScope,
) -> Result<serde_json::Value, AppError> {
    let link = repos
        .upload_link
        .find_by_token(token)
        .await?
        .ok_or_else(|| AppError::NotFound("Upload link not found".into()))?;
    if link.creator_id != user_id || !scope.allows(&link.repo_id) {
        return Err(AppError::NotFound("Upload link not found".into()));
    }

    let repo_model = repos
        .repo
        .find_by_id(&link.repo_id)
        .await?
        .ok_or_else(|| AppError::NotFound("Upload link not found".into()))?;

    let obj_name = upload_link_obj_name(&link.path);

    Ok(serde_json::json!({
        "token": link.token,
        "repo_id": link.repo_id,
        "repo_name": repo_model.name,
        "path": link.path,
        "obj_name": obj_name,
        "view_cnt": link.view_cnt,
        "ctime": link.created_at,
        "link": upload_link_url(&link.token, base_url),
        "username": "",  // placeholder — username not stored directly on upload links
        "expire_date": link.expires_at,
        "is_expired": link.expires_at.is_some_and(|exp| chrono::Utc::now().timestamp() > exp),
        "has_password": link.password.is_some(),
        "description": link.description,
    }))
}

pub async fn update_upload_link_v21(
    repos: &Repositories,
    config: &Config,
    token: &str,
    user_id: i32,
    expire_days: Option<Option<i64>>,
    password: Option<Option<String>>,
    description: Option<Option<String>>,
    scope: &RepoScope,
) -> Result<bool, AppError> {
    let link = repos
        .upload_link
        .find_by_token(token)
        .await?
        .ok_or_else(|| AppError::NotFound("Upload link not found".into()))?;
    if link.creator_id != user_id || !scope.allows(&link.repo_id) {
        return Err(AppError::NotFound("Upload link not found".into()));
    }

    let expire_at: Option<Option<i64>> = if let Some(Some(days)) = expire_days {
        let now = chrono::Utc::now().timestamp();
        Some(Some(now + days * 86400))
    } else {
        // Pass through: None = don't change, Some(None) = clear expiry
        expire_days.map(|_| None)
    };

    // Hash password if it's a new value (Some(Some(pwd)))
    let password_hash = match password {
        Some(Some(pwd)) => Some(Some(
            hash_password_async(pwd, config.auth.password_hash_iterations).await,
        )),
        Some(None) => Some(None), // explicitly clearing password
        None => None,             // don't change
    };

    repos
        .upload_link
        .update(token, user_id, expire_at, password_hash, description)
        .await
}

pub async fn list_upload_links_for_repo_v21(
    repos: &Repositories,
    base_url: &str,
    repo_id: &str,
    creator_id: i32,
) -> Result<Vec<serde_json::Value>, AppError> {
    let links = repos.upload_link.find_by_repo_id(repo_id).await?;

    let repo_name = repos
        .repo
        .find_by_id(repo_id)
        .await?
        .map(|r| r.name)
        .unwrap_or_default();

    let items: Vec<serde_json::Value> = links
        .into_iter()
        .filter(|l| l.creator_id == creator_id)
        .map(|l| {
            let obj_name = upload_link_obj_name(&l.path);

            serde_json::json!({
                "token": l.token,
                "repo_id": l.repo_id,
                "repo_name": &repo_name,
                "path": l.path,
                "obj_name": obj_name,
                "link": upload_link_url(&l.token, base_url),
                "view_cnt": l.view_cnt,
                "ctime": l.created_at,
                "expire_date": l.expires_at,
                "is_expired": l.expires_at.is_some_and(|exp| chrono::Utc::now().timestamp() > exp),
                "has_password": l.password.is_some(),
                "description": l.description,
            })
        })
        .collect();

    Ok(items)
}

pub async fn clean_invalid_upload_links_v21(
    repos: &Repositories,
    user_id: i32,
    scope: &RepoScope,
) -> Result<i32, AppError> {
    let links = repos.upload_link.find_by_creator_id(user_id).await?;

    // Batch-check repo existence in one query instead of one per link.
    let repo_ids: Vec<String> = links.iter().map(|l| l.repo_id.clone()).collect();
    let repo_rows = repos.repo.find_by_ids(&repo_ids).await?;
    let existing: std::collections::HashSet<&str> =
        repo_rows.iter().map(|r| r.id.as_str()).collect();

    let mut deleted = 0i32;

    for link in &links {
        if !scope.allows(&link.repo_id) {
            continue;
        }
        let mut should_delete = false;

        if let Some(exp) = link.expires_at
            && chrono::Utc::now().timestamp() > exp
        {
            should_delete = true;
        }

        if !should_delete && !existing.contains(link.repo_id.as_str()) {
            should_delete = true;
        }

        if should_delete {
            repos
                .upload_link
                .delete_by_id_and_user(link.id, user_id)
                .await?;
            deleted += 1;
        }
    }

    Ok(deleted)
}

/// Increment view count for an upload link (fires async, best-effort).
pub fn increment_upload_view_cnt(repos: Arc<Repositories>, link_id: i32) {
    tokio::spawn(async move {
        let _ = repos.upload_link.increment_view_cnt(link_id).await;
    });
}

pub fn get_smart_link(base_url: &str, repo_id: &str, path: &str) -> String {
    format!("{}/api2/repos/{}/file/?p={}", base_url, repo_id, path)
}
