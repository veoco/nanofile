use serde::Serialize;
use std::sync::Arc;

use crate::Config;
use crate::repository::Repositories;
use crate::service::auth::password::hash_password;
use crate::service::auth::token::{generate_share_link_token, generate_upload_link_token};
use base::error::AppError;

// ── Response types ────────────────────────────────────────────────────

#[derive(Serialize)]
pub struct UploadLinkInfo {
    pub token: String,
    pub link: String,
    pub repo_id: String,
    pub path: String,
    pub created_at: i64,
}

// ── Upload link operations (v2) ───────────────────────────────────────

pub async fn list_upload_links(
    repos: &Repositories,
    user_id: i32,
) -> Result<Vec<UploadLinkInfo>, AppError> {
    let links = repos.upload_link.find_by_creator_id(user_id).await?;

    let infos: Vec<UploadLinkInfo> = links
        .into_iter()
        .map(|l| UploadLinkInfo {
            token: l.token.clone(),
            link: format!("/u/{}/", l.token),
            repo_id: l.repo_id,
            path: l.path,
            created_at: l.created_at,
        })
        .collect();

    Ok(infos)
}

pub async fn create_upload_link(
    repos: &Repositories,
    config: &Config,
    repo_id: &str,
    path: &str,
    password: Option<&str>,
    expires_at: Option<i64>,
    creator_id: i32,
) -> Result<UploadLinkInfo, AppError> {
    let token = generate_upload_link_token();
    let now = chrono::Utc::now().timestamp();

    let password_hash = password.map(|p| hash_password(p, config.auth.password_hash_iterations));

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
            description: None,
        })
        .await?;

    Ok(UploadLinkInfo {
        token: token.clone(),
        link: format!("/u/{}/", token),
        repo_id: repo_id.to_string(),
        path: path.to_string(),
        created_at: now,
    })
}

pub async fn delete_upload_link(
    repos: &Repositories,
    token: &str,
    user_id: i32,
) -> Result<(), AppError> {
    repos
        .upload_link
        .delete_by_token_and_user(token, user_id)
        .await?;
    Ok(())
}

// ── Upload link operations (v21) ──────────────────────────────────────

pub async fn list_upload_links_v21(
    repos: &Repositories,
    user_id: i32,
) -> Result<Vec<serde_json::Value>, AppError> {
    let links = repos.upload_link.find_by_creator_id(user_id).await?;

    let items: Vec<serde_json::Value> = links
        .into_iter()
        .map(|l| {
            serde_json::json!({
                "token": l.token,
                "repo_id": l.repo_id,
                "path": l.path,
                "has_password": l.password.is_some(),
                "expire_at": l.expires_at,
                "view_cnt": l.view_cnt,
                "description": l.description,
            })
        })
        .collect();

    Ok(items)
}

pub async fn list_upload_links_for_path(
    repos: &Repositories,
    repo_id: &str,
    path: &str,
) -> Result<Vec<serde_json::Value>, AppError> {
    let links = repos
        .upload_link
        .find_by_repo_and_path(repo_id, path)
        .await?;
    let items: Vec<serde_json::Value> = links
        .into_iter()
        .map(|l| {
            serde_json::json!({
                "token": l.token,
                "repo_id": l.repo_id,
                "path": l.path,
                "has_password": l.password.is_some(),
                "expire_at": l.expires_at,
                "view_cnt": l.view_cnt,
                "description": l.description,
            })
        })
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
) -> Result<UploadLinkInfo, AppError> {
    let token = generate_share_link_token();
    let now = chrono::Utc::now().timestamp();

    let password_hash = password.map(|p| hash_password(&p, config.auth.password_hash_iterations));

    repos
        .upload_link
        .create_upload_link(crate::repository::upload_link::CreateUploadLinkParams {
            repo_id: repo_id.to_string(),
            creator_id,
            path: path.to_string(),
            token: token.clone(),
            password: password_hash,
            expires_at: expire_days.map(|d| now + d * 86400),
            created_at: now,
            description,
        })
        .await?;

    Ok(UploadLinkInfo {
        token: token.clone(),
        link: format!("/u/{}/", token),
        repo_id: repo_id.to_string(),
        path: path.to_string(),
        created_at: now,
    })
}

pub async fn delete_upload_link_v21(
    repos: &Repositories,
    id: i32,
    user_id: i32,
) -> Result<bool, AppError> {
    let result = repos.upload_link.delete_by_id_and_user(id, user_id).await?;
    Ok(result.rows_affected > 0)
}

/// Delete an upload link by token string (seahub-compatible).
pub async fn delete_upload_link_v21_by_token(
    repos: &Repositories,
    token: &str,
    user_id: i32,
) -> Result<bool, AppError> {
    // Find the link by token first, then delete by id
    let link = repos
        .upload_link
        .find_by_token(token)
        .await?
        .ok_or_else(|| AppError::NotFound("Upload link not found".into()))?;

    if link.creator_id != user_id {
        return Err(AppError::Forbidden);
    }

    let result = repos
        .upload_link
        .delete_by_id_and_user(link.id, user_id)
        .await?;
    Ok(result.rows_affected > 0)
}

pub async fn get_upload_link_v21(
    repos: &Repositories,
    token: &str,
) -> Result<serde_json::Value, AppError> {
    let link = repos
        .upload_link
        .find_by_token(token)
        .await?
        .ok_or_else(|| AppError::NotFound("Upload link not found".into()))?;

    // Check if repo still exists
    let repo_model = repos
        .repo
        .find_by_id(&link.repo_id)
        .await?
        .ok_or_else(|| AppError::NotFound("Upload link not found".into()))?;

    let obj_name = if link.path == "/" {
        "/".to_string()
    } else {
        link.path
            .trim_end_matches('/')
            .rsplit_once('/')
            .map(|(_, n)| n.to_string())
            .unwrap_or_else(|| link.path.clone())
    };

    Ok(serde_json::json!({
        "token": link.token,
        "repo_id": link.repo_id,
        "repo_name": repo_model.name,
        "path": link.path,
        "obj_name": obj_name,
        "view_cnt": link.view_cnt,
        "ctime": link.created_at,
        "link": format!("/u/{}/", link.token),
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
) -> Result<bool, AppError> {
    // Convert expire_days to expire_at timestamp
    let expire_at: Option<Option<i64>> = if let Some(Some(days)) = expire_days {
        let now = chrono::Utc::now().timestamp();
        Some(Some(now + days * 86400))
    } else {
        // Pass through: None = don't change, Some(None) = clear expiry
        expire_days.map(|_| None)
    };

    // Hash password if it's a new value (Some(Some(pwd)))
    let password_hash = match password {
        Some(Some(pwd)) => Some(Some(hash_password(
            &pwd,
            config.auth.password_hash_iterations,
        ))),
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
    repo_id: &str,
) -> Result<Vec<serde_json::Value>, AppError> {
    let links = repos.upload_link.find_by_repo_id(repo_id).await?;

    // Look up repo name
    let repo_name = repos
        .repo
        .find_by_id(repo_id)
        .await?
        .map(|r| r.name)
        .unwrap_or_default();

    let items: Vec<serde_json::Value> = links
        .into_iter()
        .map(|l| {
            let obj_name = if l.path == "/" {
                "/".to_string()
            } else {
                l.path
                    .trim_end_matches('/')
                    .rsplit_once('/')
                    .map(|(_, n)| n.to_string())
                    .unwrap_or_else(|| l.path.clone())
            };

            serde_json::json!({
                "token": l.token,
                "repo_id": l.repo_id,
                "repo_name": &repo_name,
                "path": l.path,
                "obj_name": obj_name,
                "link": format!("/u/{}/", l.token),
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
) -> Result<i32, AppError> {
    let links = repos.upload_link.find_by_creator_id(user_id).await?;
    let mut deleted = 0i32;

    for link in &links {
        let mut should_delete = false;

        // Check if expired
        if let Some(exp) = link.expires_at
            && chrono::Utc::now().timestamp() > exp
        {
            should_delete = true;
        }

        // Check if repo still exists
        if !should_delete {
            let repo_exists = repos.repo.find_by_id(&link.repo_id).await?.is_some();
            if !repo_exists {
                should_delete = true;
            }
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

// ── Smart link ────────────────────────────────────────────────────────

pub fn get_smart_link(base_url: &str, repo_id: &str, path: &str) -> String {
    format!("{}/api2/repos/{}/file/?p={}", base_url, repo_id, path)
}
