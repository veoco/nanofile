use axum::{
    Json, Router,
    extract::{Form, Path, Query, State},
    http::StatusCode,
};
use sea_orm::{ActiveModelTrait, ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter, Set};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;

use crate::AppState;
use crate::auth::middleware::AuthUser;
use crate::auth::token::generate_sync_token;
use crate::entity::{repo, repo_member, sync_token, user};
use crate::error::AppError;

#[derive(Deserialize)]
pub struct CreateRepoRequest {
    pub name: String,
    pub desc: Option<String>,
    pub repo_id: Option<String>,
    pub encrypted: Option<i32>,
    pub enc_version: Option<i32>,
    pub magic: Option<String>,
    pub random_key: Option<String>,
}

#[derive(Serialize)]
pub struct RepoInfo {
    pub id: String,
    pub name: String,
    pub desc: String,
    #[serde(rename = "owner")]
    pub owner: String,
    pub encrypted: bool,
    #[serde(rename = "enc_version")]
    pub enc_version: i32,
    pub size: i64,
    pub mtime: i64,
    #[serde(rename = "permission")]
    pub permission: String,
    #[serde(rename = "head_commit_id")]
    pub head_commit_id: Option<String>,
    #[serde(rename = "type")]
    pub type_: String,
    #[serde(rename = "virtual")]
    pub virtual_: bool,
    pub root: Option<String>,
    pub salt: Option<String>,
    pub repo_version: i32,
    /// Extra fields set only in create-repo responses.
    /// The Qt client parses POST /api2/repos/ response as RepoDownloadInfo,
    /// which expects repo_id, repo_name, token, email fields.
    #[serde(rename = "repo_id", skip_serializing_if = "Option::is_none")]
    pub repo_id_dup: Option<String>,
    #[serde(rename = "repo_name", skip_serializing_if = "Option::is_none")]
    pub repo_name_dup: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub token: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
}

#[derive(Serialize)]
pub struct DownloadInfoResponse {
    pub repo_id: String,
    pub repo_name: String,
    pub token: String,
    pub email: String,
    pub relay_id: Option<String>,
    pub relay_addr: Option<String>,
    pub relay_port: Option<String>,
    pub enc_version: i32,
    pub encrypted: String,
    pub magic: Option<String>,
    pub random_key: Option<String>,
    pub repo_version: i32,
    pub salt: Option<String>,
    pub permission: String,
}

pub fn repo_routes() -> Router<Arc<AppState>> {
    Router::new()
        .route(
            "/{repo_id}/",
            axum::routing::get(get_repo).delete(delete_repo),
        )
        .route(
            "/{repo_id}/download-info/",
            axum::routing::get(download_info),
        )
        .route(
            "/{repo_id}/upload-link/",
            axum::routing::get(get_upload_link),
        )
        .route(
            "/{repo_id}/update-link/",
            axum::routing::get(get_update_link),
        )
}

pub async fn list_repos(
    auth: AuthUser,
    State(state): State<Arc<AppState>>,
) -> Result<Json<Vec<RepoInfo>>, AppError> {
    let memberships = repo_member::Entity::find()
        .filter(repo_member::Column::UserId.eq(auth.user_id))
        .all(state.db.as_ref())
        .await?;

    let mut repos = Vec::new();
    for m in memberships {
        if let Some(r) = repo::Entity::find_by_id(&m.repo_id)
            .one(state.db.as_ref())
            .await?
        {
            repos.push(RepoInfo {
                id: r.id.clone(),
                name: r.name.clone(),
                desc: r.description.clone(),
                owner: auth.email.clone(),
                encrypted: r.encrypted != 0,
                enc_version: r.enc_version as i32,
                size: 0,
                mtime: r.updated_at,
                permission: r.permission.clone(),
                head_commit_id: r.head_commit_id.clone(),
                type_: "repo".to_string(),
                virtual_: false,
                root: None,
                salt: None,
                repo_version: r.repo_version,
                repo_id_dup: None,
                repo_name_dup: None,
                token: None,
                email: None,
            });
        }
    }

    Ok(Json(repos))
}

pub async fn create_repo(
    auth: AuthUser,
    State(state): State<Arc<AppState>>,
    Form(req): Form<CreateRepoRequest>,
) -> Result<(StatusCode, Json<RepoInfo>), AppError> {
    let repo_id = req
        .repo_id
        .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
    let now = chrono::Utc::now().timestamp();

    let desc = req.desc.unwrap_or_default();
    let encrypted_val = req.encrypted.unwrap_or(0);
    let enc_version_val = req.enc_version.unwrap_or(0);

    let model = repo::ActiveModel {
        id: sea_orm::Set(repo_id.clone()),
        name: sea_orm::Set(req.name.clone()),
        description: sea_orm::Set(desc.clone()),
        owner_id: sea_orm::Set(auth.user_id),
        encrypted: sea_orm::Set(encrypted_val as i8),
        enc_version: sea_orm::Set(enc_version_val as i8),
        magic: sea_orm::Set(req.magic),
        random_key: sea_orm::Set(req.random_key),
        salt: sea_orm::Set(String::new()),
        head_commit_id: sea_orm::NotSet,
        permission: sea_orm::Set("rw".to_string()),
        repo_version: sea_orm::Set(1),
        size: sea_orm::Set(0),
        created_at: sea_orm::Set(now),
        updated_at: sea_orm::Set(now),
    };
    repo::Entity::insert(model).exec(state.db.as_ref()).await?;

    let member = repo_member::ActiveModel {
        id: sea_orm::NotSet,
        repo_id: sea_orm::Set(repo_id.clone()),
        user_id: sea_orm::Set(auth.user_id),
        permission: sea_orm::Set("rw".to_string()),
        created_at: sea_orm::Set(now),
    };
    repo_member::Entity::insert(member)
        .exec(state.db.as_ref())
        .await?;

    // Generate a sync token — the Qt client parses the create response
    // as RepoDownloadInfo and immediately calls cloneRepo() with it.
    let token_value = crate::auth::token::generate_sync_token();
    let sync_token_model = sync_token::ActiveModel {
        id: sea_orm::NotSet,
        repo_id: sea_orm::Set(repo_id.clone()),
        user_id: sea_orm::Set(auth.user_id),
        token: sea_orm::Set(token_value.clone()),
        created_at: sea_orm::Set(now),
        expires_at: sea_orm::Set(None),
        peer_id: sea_orm::NotSet,
        peer_name: sea_orm::NotSet,
        peer_ip: sea_orm::NotSet,
        client_version: sea_orm::NotSet,
        last_sync_time: sea_orm::NotSet,
    };
    sync_token::Entity::insert(sync_token_model)
        .exec(state.db.as_ref())
        .await?;

    Ok((
        StatusCode::CREATED,
        Json(RepoInfo {
            id: repo_id.clone(),
            name: req.name.clone(),
            desc,
            owner: auth.email.clone(),
            encrypted: encrypted_val == 1,
            enc_version: enc_version_val,
            size: 0,
            mtime: now,
            permission: "rw".to_string(),
            head_commit_id: None,
            type_: "repo".to_string(),
            virtual_: false,
            root: None,
            salt: None,
            repo_version: 1,
            // Extra fields for RepoDownloadInfo compatibility
            repo_id_dup: Some(repo_id),
            repo_name_dup: Some(req.name.clone()),
            token: Some(token_value),
            email: Some(auth.email),
        }),
    ))
}

pub async fn get_repo(
    auth: AuthUser,
    State(state): State<Arc<AppState>>,
    Path(repo_id): Path<String>,
) -> Result<Json<RepoInfo>, AppError> {
    let r = repo::Entity::find_by_id(&repo_id)
        .one(state.db.as_ref())
        .await?
        .ok_or_else(|| AppError::NotFound("repo not found".into()))?;

    Ok(Json(RepoInfo {
        id: r.id.clone(),
        name: r.name.clone(),
        desc: r.description.clone(),
        owner: auth.email,
        encrypted: r.encrypted != 0,
        enc_version: r.enc_version as i32,
        size: 0,
        mtime: r.updated_at,
        permission: r.permission.clone(),
        head_commit_id: r.head_commit_id.clone(),
        type_: "repo".to_string(),
        virtual_: false,
        root: None,
        salt: None,
        repo_version: r.repo_version,
        repo_id_dup: None,
        repo_name_dup: None,
        token: None,
        email: None,
    }))
}

pub async fn delete_repo(
    _auth: AuthUser,
    State(state): State<Arc<AppState>>,
    Path(repo_id): Path<String>,
) -> Result<(), AppError> {
    repo::Entity::delete_by_id(&repo_id)
        .exec(state.db.as_ref())
        .await?;
    Ok(())
}

pub async fn download_info(
    auth: AuthUser,
    State(state): State<Arc<AppState>>,
    Path(repo_id): Path<String>,
) -> Result<Json<DownloadInfoResponse>, AppError> {
    let r = repo::Entity::find_by_id(&repo_id)
        .one(state.db.as_ref())
        .await?
        .ok_or_else(|| AppError::NotFound("repo not found".into()))?;

    let u = user::Entity::find_by_id(auth.user_id)
        .one(state.db.as_ref())
        .await?
        .ok_or_else(|| AppError::NotFound("user not found".into()))?;

    let token_value = ensure_sync_token(state.db.as_ref(), &repo_id, auth.user_id).await?;

    Ok(Json(DownloadInfoResponse {
        repo_id,
        repo_name: r.name,
        token: token_value,
        email: u.email,
        relay_id: None,
        relay_addr: None,
        relay_port: None,
        enc_version: r.enc_version as i32,
        encrypted: if r.encrypted == 1 {
            "true".to_string()
        } else {
            "false".to_string()
        },
        magic: r.magic,
        random_key: r.random_key,
        repo_version: 1,
        salt: if r.salt.is_empty() {
            None
        } else {
            Some(r.salt.clone())
        },
        permission: r.permission,
    }))
}

#[derive(Deserialize)]
pub struct LinkQuery {
    pub p: Option<String>,
    /// `from=web` returns an `upload-aj` (or `update-aj`) URL;
    /// `from=api` (or absent) returns an `upload-api` (or `update-api`) URL.
    pub from: Option<String>,
    /// Optional replace flag (only used when `from=api`).
    pub replace: Option<String>,
}

/// Build a file-server URL for the given operation (e.g. "upload-aj", "upload-api").
fn build_op_url(state: &AppState, op: &str, token: &str) -> String {
    let host = if state.config.server.addr == "0.0.0.0"
        || state.config.server.addr == "::"
        || state.config.server.addr == "127.0.0.1"
    {
        "127.0.0.1"
    } else {
        &state.config.server.addr
    };
    format!(
        "http://{}:{}/{}/{}",
        host, state.config.server.port, op, token
    )
}

pub async fn get_upload_link(
    auth: AuthUser,
    State(state): State<Arc<AppState>>,
    Path(repo_id): Path<String>,
    Query(query): Query<LinkQuery>,
) -> Result<Json<String>, AppError> {
    let parent_dir = query.p.as_deref().unwrap_or("/");

    // Verify repo exists
    repo::Entity::find_by_id(&repo_id)
        .one(state.db.as_ref())
        .await?
        .ok_or_else(|| AppError::NotFound("repo not found".into()))?;

    let token =
        state
            .token_manager
            .generate(&repo_id, auth.user_id, &auth.email, "upload", parent_dir);

    // Choose the right operation based on the `from` parameter.
    // from=web → upload-aj (AJAX/web client), from=api (or absent) → upload-api.
    let is_web = query.from.as_deref() == Some("web");
    let op = if is_web { "upload-aj" } else { "upload-api" };
    let mut url = build_op_url(&state, op, &token);

    // When from=api, optionally append ?replace=1
    if !is_web && query.replace.as_deref() == Some("1") {
        url.push_str("?replace=1");
    }

    Ok(Json(url))
}

pub async fn get_update_link(
    auth: AuthUser,
    State(state): State<Arc<AppState>>,
    Path(repo_id): Path<String>,
    Query(query): Query<LinkQuery>,
) -> Result<Json<String>, AppError> {
    let parent_dir = query.p.as_deref().unwrap_or("/");

    // Verify repo exists
    repo::Entity::find_by_id(&repo_id)
        .one(state.db.as_ref())
        .await?
        .ok_or_else(|| AppError::NotFound("repo not found".into()))?;

    let token =
        state
            .token_manager
            .generate(&repo_id, auth.user_id, &auth.email, "update", parent_dir);

    // Choose the right operation based on the `from` parameter.
    let op = if query.from.as_deref() == Some("web") {
        "update-aj"
    } else {
        "update-api"
    };
    let url = build_op_url(&state, op, &token);

    Ok(Json(url))
}

/// Ensure a sync token exists for the given user+repo pair.
async fn ensure_sync_token(
    db: &DatabaseConnection,
    repo_id: &str,
    user_id: i32,
) -> Result<String, AppError> {
    // Check if a sync token already exists
    if let Some(existing) = sync_token::Entity::find()
        .filter(sync_token::Column::RepoId.eq(repo_id))
        .filter(sync_token::Column::UserId.eq(user_id))
        .one(db)
        .await?
    {
        return Ok(existing.token);
    }

    // Create a new one
    let token_value = generate_sync_token();
    let now = chrono::Utc::now().timestamp();
    sync_token::ActiveModel {
        id: sea_orm::NotSet,
        repo_id: Set(repo_id.to_string()),
        user_id: Set(user_id),
        token: Set(token_value.clone()),
        created_at: Set(now),
        expires_at: Set(None),
        peer_id: sea_orm::NotSet,
        peer_name: sea_orm::NotSet,
        peer_ip: sea_orm::NotSet,
        client_version: sea_orm::NotSet,
        last_sync_time: sea_orm::NotSet,
    }
    .insert(db)
    .await?;

    Ok(token_value)
}

/// `GET /api2/repo-tokens/?repos=id1,id2`
///
/// Batch get sync tokens for multiple repos.
pub async fn repo_tokens(
    auth: AuthUser,
    State(state): State<Arc<AppState>>,
    Query(params): Query<HashMap<String, String>>,
) -> Result<Json<HashMap<String, String>>, AppError> {
    let repos_param = params
        .get("repos")
        .ok_or_else(|| AppError::BadRequest("repos parameter required".into()))?;
    let repo_ids: Vec<&str> = repos_param
        .split(',')
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .collect();

    let mut result = HashMap::new();
    for repo_id in &repo_ids {
        let token = ensure_sync_token(state.db.as_ref(), repo_id, auth.user_id).await?;
        result.insert(repo_id.to_string(), token);
    }

    Ok(Json(result))
}

/// `GET /api2/default-repo/`
///
/// Returns the user's default repo (the first repo they created).
pub async fn get_default_repo(
    auth: AuthUser,
    State(state): State<Arc<AppState>>,
) -> Result<Json<RepoInfo>, AppError> {
    let membership = repo_member::Entity::find()
        .filter(repo_member::Column::UserId.eq(auth.user_id))
        .one(state.db.as_ref())
        .await?;

    match membership {
        Some(m) => {
            let r = repo::Entity::find_by_id(&m.repo_id)
                .one(state.db.as_ref())
                .await?;
            match r {
                Some(r) => Ok(Json(RepoInfo {
                    id: r.id.clone(),
                    name: r.name.clone(),
                    desc: r.description.clone(),
                    owner: auth.email,
                    encrypted: r.encrypted != 0,
                    enc_version: r.enc_version as i32,
                    size: r.size,
                    mtime: r.updated_at,
                    permission: r.permission.clone(),
                    head_commit_id: r.head_commit_id.clone(),
                    type_: "repo".to_string(),
                    virtual_: false,
                    root: None,
                    salt: None,
                    repo_version: r.repo_version,
                    repo_id_dup: None,
                    repo_name_dup: None,
                    token: None,
                    email: None,
                })),
                None => Err(AppError::NotFound("no default repo".into())),
            }
        }
        None => Err(AppError::NotFound("no default repo".into())),
    }
}

/// `POST /api2/default-repo/`
///
/// Creates a default repo for the user (named "我的资料库").
pub async fn create_default_repo(
    auth: AuthUser,
    State(state): State<Arc<AppState>>,
) -> Result<(StatusCode, Json<RepoInfo>), AppError> {
    // Check if user already has a repo — return it if so
    let existing_member = repo_member::Entity::find()
        .filter(repo_member::Column::UserId.eq(auth.user_id))
        .one(state.db.as_ref())
        .await?;

    if let Some(m) = existing_member {
        if let Some(r) = repo::Entity::find_by_id(&m.repo_id)
            .one(state.db.as_ref())
            .await?
        {
            return Ok((
                StatusCode::OK,
                Json(RepoInfo {
                    id: r.id.clone(),
                    name: r.name.clone(),
                    desc: r.description.clone(),
                    owner: auth.email,
                    encrypted: r.encrypted != 0,
                    enc_version: r.enc_version as i32,
                    size: r.size,
                    mtime: r.updated_at,
                    permission: r.permission.clone(),
                    head_commit_id: r.head_commit_id.clone(),
                    type_: "repo".to_string(),
                    virtual_: false,
                    root: None,
                    salt: None,
                    repo_version: r.repo_version,
                    repo_id_dup: None,
                    repo_name_dup: None,
                    token: None,
                    email: None,
                }),
            ));
        }
        // Stale repo_member — the repo was deleted, clean up and continue
        repo_member::Entity::delete_by_id(m.id)
            .exec(state.db.as_ref())
            .await?;
    }

    // Create a new repo as default
    let repo_id = uuid::Uuid::new_v4().to_string();
    let now = chrono::Utc::now().timestamp();

    let model = repo::ActiveModel {
        id: sea_orm::Set(repo_id.clone()),
        name: sea_orm::Set("我的资料库".to_string()),
        description: sea_orm::Set(String::new()),
        owner_id: sea_orm::Set(auth.user_id),
        encrypted: sea_orm::Set(0i8),
        enc_version: sea_orm::Set(0i8),
        magic: sea_orm::Set(None),
        random_key: sea_orm::Set(None),
        salt: sea_orm::Set(String::new()),
        head_commit_id: sea_orm::NotSet,
        permission: sea_orm::Set("rw".to_string()),
        repo_version: sea_orm::Set(1),
        size: sea_orm::Set(0),
        created_at: sea_orm::Set(now),
        updated_at: sea_orm::Set(now),
    };
    repo::Entity::insert(model).exec(state.db.as_ref()).await?;

    repo_member::Entity::insert(repo_member::ActiveModel {
        id: sea_orm::NotSet,
        repo_id: sea_orm::Set(repo_id.clone()),
        user_id: sea_orm::Set(auth.user_id),
        permission: sea_orm::Set("rw".to_string()),
        created_at: sea_orm::Set(now),
    })
    .exec(state.db.as_ref())
    .await?;

    let token_value = crate::auth::token::generate_sync_token();
    sync_token::Entity::insert(sync_token::ActiveModel {
        id: sea_orm::NotSet,
        repo_id: sea_orm::Set(repo_id.clone()),
        user_id: sea_orm::Set(auth.user_id),
        token: sea_orm::Set(token_value.clone()),
        created_at: sea_orm::Set(now),
        expires_at: sea_orm::Set(None),
        peer_id: sea_orm::NotSet,
        peer_name: sea_orm::NotSet,
        peer_ip: sea_orm::NotSet,
        client_version: sea_orm::NotSet,
        last_sync_time: sea_orm::NotSet,
    })
    .exec(state.db.as_ref())
    .await?;

    Ok((
        StatusCode::CREATED,
        Json(RepoInfo {
            id: repo_id.clone(),
            name: "我的资料库".to_string(),
            desc: String::new(),
            owner: auth.email.clone(),
            encrypted: false,
            enc_version: 0,
            size: 0,
            mtime: now,
            permission: "rw".to_string(),
            head_commit_id: None,
            type_: "repo".to_string(),
            virtual_: false,
            root: None,
            salt: None,
            repo_version: 1,
            repo_id_dup: Some(repo_id),
            repo_name_dup: Some("我的资料库".to_string()),
            token: Some(token_value),
            email: Some(auth.email),
        }),
    ))
}
