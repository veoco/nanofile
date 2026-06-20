use sea_orm::{DatabaseConnection, EntityTrait, Set};
use serde::Serialize;

use crate::auth::password::hash_password;
use crate::auth::token::{generate_share_link_token, generate_upload_link_token};
use crate::entity::upload_link;
use crate::error::AppError;
use crate::repository::Repositories;
use crate::Config;

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
    db: &DatabaseConnection,
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

    let model = upload_link::ActiveModel {
        id: sea_orm::NotSet,
        repo_id: Set(repo_id.to_string()),
        creator_id: Set(creator_id),
        path: Set(path.to_string()),
        token: Set(token.clone()),
        password: Set(password_hash),
        expires_at: Set(expires_at),
        created_at: Set(now),
        view_cnt: Set(0i64),
        description: Set(None),
    };
    upload_link::Entity::insert(model).exec(db).await?;

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

pub async fn create_upload_link_v21(
    db: &DatabaseConnection,
    repo_id: &str,
    path: &str,
    password: Option<String>,
    expire_days: Option<i64>,
    creator_id: i32,
) -> Result<String, AppError> {
    let token = generate_share_link_token();
    let now = chrono::Utc::now().timestamp();

    upload_link::Entity::insert(upload_link::ActiveModel {
        id: sea_orm::NotSet,
        repo_id: Set(repo_id.to_string()),
        creator_id: Set(creator_id),
        path: Set(path.to_string()),
        token: Set(token.clone()),
        password: Set(password),
        expires_at: Set(expire_days.map(|d| now + d * 86400)),
        created_at: Set(now),
        view_cnt: Set(0i64),
        description: Set(None),
    })
    .exec(db)
    .await?;

    Ok(token)
}

pub async fn delete_upload_link_v21(
    repos: &Repositories,
    id: i32,
    user_id: i32,
) -> Result<bool, AppError> {
    let result = repos.upload_link.delete_by_id_and_user(id, user_id).await?;
    Ok(result.rows_affected > 0)
}

// ── Smart link ────────────────────────────────────────────────────────

pub fn get_smart_link(base_url: &str, repo_id: &str, path: &str) -> String {
    format!(
        "{}/api2/repos/{}/file/?p={}",
        base_url, repo_id, path
    )
}
