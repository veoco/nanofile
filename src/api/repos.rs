use axum::{
    Json, Router,
    body::{Body, Bytes},
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
};
use sea_orm::{ActiveModelTrait, ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter, Set};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;

use crate::AppState;
use crate::activity_log;
use crate::auth::middleware::AuthUser;
use crate::auth::token::generate_sync_token;
use crate::entity::{commit, repo, repo_member, sync_token, user};
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
    #[serde(rename = "enc_version", skip_serializing_if = "Option::is_none")]
    pub enc_version: Option<i32>,
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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub magic: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub random_key: Option<String>,
    pub repo_version: i32,
    /// Indicates whether the user needs to decrypt this encrypted repo.
    /// Only present when the repo is encrypted. Matches seahub's
    /// `lib_need_decrypt` field in `api2/views.py::RepoView`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lib_need_decrypt: Option<bool>,
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
            axum::routing::get(get_repo)
                .post(repo_post_handler)
                .delete(delete_repo),
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
            let encrypted = r.encrypted != 0;
            repos.push(RepoInfo {
                id: r.id.clone(),
                name: r.name.clone(),
                desc: r.description.clone(),
                owner: auth.email.clone(),
                encrypted,
                enc_version: if encrypted {
                    Some(r.enc_version as i32)
                } else {
                    None
                },
                size: r.size,
                mtime: r.updated_at,
                permission: m.permission.clone(),
                head_commit_id: r.head_commit_id.clone(),
                type_: if r.owner_id == auth.user_id {
                    "repo".to_string()
                } else {
                    "srepo".to_string()
                },
                virtual_: false,
                root: None,
                salt: if encrypted && r.enc_version >= 3 {
                    Some(r.salt.clone())
                } else {
                    None
                },
                magic: if encrypted { r.magic.clone() } else { None },
                random_key: if encrypted {
                    r.random_key.clone()
                } else {
                    None
                },
                repo_version: r.repo_version,
                lib_need_decrypt: if encrypted { Some(true) } else { None },
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
    headers: HeaderMap,
    bytes: Bytes,
) -> Result<(StatusCode, Json<RepoInfo>), AppError> {
    // Support JSON (web frontend), form-encoded (desktop client), and
    // multipart/form-data (Android client) bodies.
    let repo_req: CreateRepoRequest = if headers
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .is_some_and(|ct| ct.contains("json"))
    {
        serde_json::from_slice(&bytes)?
    } else if let Ok(form) = serde_urlencoded::from_bytes::<CreateRepoRequest>(&bytes) {
        form
    } else {
        // multipart/form-data – scan for field names in the raw body string.
        let name = extract_multipart_field(&bytes, "name")
            .ok_or_else(|| AppError::BadRequest("name required".into()))?;
        let desc = extract_multipart_field(&bytes, "desc");
        CreateRepoRequest {
            name,
            desc,
            repo_id: None,
            encrypted: None,
            enc_version: None,
            magic: None,
            random_key: None,
        }
    };

    let repo_id = repo_req
        .repo_id
        .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
    let now = chrono::Utc::now().timestamp();

    let desc = repo_req.desc.unwrap_or_default();
    let encrypted_val = repo_req.encrypted.unwrap_or(0);
    let enc_version_val = repo_req.enc_version.unwrap_or(0);
    let magic = repo_req.magic.clone();
    let random_key = repo_req.random_key.clone();

    let model = repo::ActiveModel {
        id: sea_orm::Set(repo_id.clone()),
        name: sea_orm::Set(repo_req.name.clone()),
        description: sea_orm::Set(desc.clone()),
        owner_id: sea_orm::Set(auth.user_id),
        encrypted: sea_orm::Set(encrypted_val as i8),
        enc_version: sea_orm::Set(enc_version_val as i8),
        magic: sea_orm::Set(magic.clone()),
        random_key: sea_orm::Set(random_key.clone()),
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

    let encrypted = encrypted_val == 1;

    // Log repo creation activity (best-effort)
    crate::activity_log::log_activity(
        state.db.as_ref(),
        &repo_id,
        "create",
        "repo",
        "/",
        auth.user_id,
        None,
        None,
        None,
        None,
        None,
    )
    .await;

    Ok((
        StatusCode::CREATED,
        Json(RepoInfo {
            id: repo_id.clone(),
            name: repo_req.name.clone(),
            desc,
            owner: auth.email.clone(),
            encrypted,
            enc_version: if encrypted {
                Some(enc_version_val)
            } else {
                None
            },
            size: 0,
            mtime: now,
            permission: "rw".to_string(),
            head_commit_id: None,
            type_: "repo".to_string(),
            virtual_: false,
            root: None,
            salt: None,
            magic: if encrypted { magic } else { None },
            random_key: if encrypted { random_key } else { None },
            repo_version: 1,
            lib_need_decrypt: if encrypted { Some(true) } else { None },
            // Extra fields for RepoDownloadInfo compatibility
            repo_id_dup: Some(repo_id),
            repo_name_dup: Some(repo_req.name.clone()),
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

    // Look up membership to get the user's effective permission.
    let membership = repo_member::Entity::find()
        .filter(repo_member::Column::RepoId.eq(&repo_id))
        .filter(repo_member::Column::UserId.eq(auth.user_id))
        .one(state.db.as_ref())
        .await?;
    let permission = membership
        .map(|m| m.permission)
        .unwrap_or_else(|| "rw".to_string());

    // Load head commit to get the root fs_id (root directory's SHA-1), matching seahub.
    let root = if let Some(ref cmmt_id) = r.head_commit_id {
        commit::Entity::find()
            .filter(commit::Column::RepoId.eq(&r.id))
            .filter(commit::Column::CommitId.eq(cmmt_id))
            .one(state.db.as_ref())
            .await?
            .map(|c| c.root_id)
    } else {
        None
    };

    // Follow original seahub: enc_version/salt/magic/random_key only for encrypted repos
    let enc_version = if r.encrypted != 0 {
        Some(r.enc_version as i32)
    } else {
        None
    };
    let salt = if r.encrypted != 0 && r.enc_version >= 3 {
        Some(r.salt.clone())
    } else {
        None
    };
    let magic = if r.encrypted != 0 {
        r.magic.clone()
    } else {
        None
    };
    let random_key = if r.encrypted != 0 {
        r.random_key.clone()
    } else {
        None
    };

    Ok(Json(RepoInfo {
        id: r.id.clone(),
        name: r.name.clone(),
        desc: r.description.clone(),
        owner: auth.email,
        encrypted: r.encrypted != 0,
        enc_version,
        size: r.size,
        mtime: r.updated_at,
        permission,
        head_commit_id: r.head_commit_id.clone(),
        type_: if r.owner_id == auth.user_id {
            "repo".to_string()
        } else {
            "srepo".to_string()
        },
        virtual_: false,
        root,
        salt,
        magic,
        random_key,
        repo_version: r.repo_version,
        lib_need_decrypt: if r.encrypted != 0 { Some(true) } else { None },
        repo_id_dup: None,
        repo_name_dup: None,
        token: None,
        email: None,
    }))
}

/// `POST /api2/repos/{repo_id}/?op=rename`
///
/// Accepts `repo_name` from JSON, form-urlencoded, or multipart/form-data
/// (Android client sends multipart with part `name="repo_name"`).
pub async fn rename_repo(
    auth: AuthUser,
    State(state): State<Arc<AppState>>,
    Path(repo_id): Path<String>,
    Query(params): Query<HashMap<String, String>>,
    req: axum::http::Request<Body>,
) -> Result<Json<serde_json::Value>, AppError> {
    // Verify op=rename
    match params.get("op").map(|s| s.as_str()) {
        Some("rename") => {}
        _ => return Err(AppError::BadRequest("invalid operation".into())),
    }

    // Parse repo_name from body, trying JSON, form-urlencoded, then
    // multipart/form-data (Android client).
    let (_parts, body) = req.into_parts();
    let bytes = axum::body::to_bytes(body, usize::MAX)
        .await
        .map_err(|e| AppError::Internal(e.to_string()))?;

    let repo_name = parse_repo_name(&bytes)?;

    // Load repo
    let r = repo::Entity::find_by_id(&repo_id)
        .one(state.db.as_ref())
        .await?
        .ok_or_else(|| AppError::NotFound("repo not found".into()))?;

    // Only owner can rename
    if r.owner_id != auth.user_id {
        return Err(AppError::Forbidden);
    }

    // Validate name
    let new_name = repo_name.trim().to_string();
    if new_name.is_empty() || new_name.contains('/') {
        return Err(AppError::BadRequest("invalid repo name".into()));
    }

    // Log repo rename activity (before update, so detail captures the old name).
    activity_log::log_activity(
        state.db.as_ref(),
        &repo_id,
        "rename",
        "repo",
        "/",
        auth.user_id,
        None,
        None,
        None,
        Some(&r.name),
        None,
    )
    .await;

    // Update
    let now = chrono::Utc::now().timestamp();
    let mut active: repo::ActiveModel = r.into();
    active.name = Set(new_name);
    active.updated_at = Set(now);
    active.update(state.db.as_ref()).await?;

    // Return a JSON string "success" instead of a JSON object so the
    // Android client's SupportResponseConverter can parse it as String.
    // Json("success".to_string()) serializes as the JSON string "success".
    Ok(Json(serde_json::Value::String("success".to_string())))
}

/// `POST /api2/repos/{repo_id}/`
///
/// Dispatches to the appropriate handler based on the `op` query parameter.
/// Supports: rename, setpassword, checkpassword.
pub async fn repo_post_handler(
    auth: AuthUser,
    state: State<Arc<AppState>>,
    Path(repo_id): Path<String>,
    Query(params): Query<HashMap<String, String>>,
    req: axum::http::Request<Body>,
) -> Result<Json<serde_json::Value>, AppError> {
    match params.get("op").map(|s| s.as_str()) {
        Some("rename") => rename_repo(auth, state, Path(repo_id), Query(params), req).await,
        Some("setpassword") => set_repo_password_v2(auth, state, Path(repo_id), req).await,
        Some("checkpassword") => check_repo_password_v2(auth, state, Path(repo_id), req).await,
        _ => Err(AppError::BadRequest(
            "invalid operation; use rename, setpassword, or checkpassword".into(),
        )),
    }
}

/// `POST /api2/repos/{repo_id}/?op=setpassword`
///
/// Set the password for an encrypted repo (v2 API).
///
/// Expects `password` field in form-urlencoded body (matching seahub's
/// `api2/views.py:set_repo_password()` which sends `password` from a
/// form-encoded POST).
pub async fn set_repo_password_v2(
    auth: AuthUser,
    State(state): State<Arc<AppState>>,
    Path(repo_id): Path<String>,
    req: axum::http::Request<Body>,
) -> Result<Json<serde_json::Value>, AppError> {
    let (_parts, body) = req.into_parts();
    let bytes = axum::body::to_bytes(body, usize::MAX)
        .await
        .map_err(|e| AppError::Internal(e.to_string()))?;

    // Parse password from form data (JSON, urlencoded, or multipart)
    let password = parse_password_field(&bytes)?;

    crate::api_v21::repo_set_password::set_repo_password_inner(
        &state,
        &repo_id,
        auth.user_id,
        &password,
    )
    .await?;

    Ok(Json(serde_json::json!({"success": true})))
}

/// `POST /api2/repos/{repo_id}/?op=checkpassword`
///
/// Check if a password is valid for an encrypted repo (v2 API).
///
/// Expects `magic` field in form-urlencoded body (the client sends the
/// pre-computed magic string, not the raw password). The server compares
/// it against the stored magic.
pub async fn check_repo_password_v2(
    _auth: AuthUser,
    State(state): State<Arc<AppState>>,
    Path(repo_id): Path<String>,
    req: axum::http::Request<Body>,
) -> Result<Json<serde_json::Value>, AppError> {
    let (_parts, body) = req.into_parts();
    let bytes = axum::body::to_bytes(body, usize::MAX)
        .await
        .map_err(|e| AppError::Internal(e.to_string()))?;

    // Parse magic from form data
    let magic = parse_magic_field(&bytes)?;

    // Load the repo
    let repo_model = repo::Entity::find_by_id(&repo_id)
        .one(state.db.as_ref())
        .await?
        .ok_or_else(|| AppError::NotFound("repo not found".into()))?;

    if repo_model.encrypted == 0 {
        return Err(AppError::BadRequest("repo is not encrypted".into()));
    }

    let stored_magic = repo_model
        .magic
        .as_deref()
        .ok_or_else(|| AppError::BadRequest("repo has no stored magic".into()))?;

    // Use constant-time comparison
    use crate::crypto::verify::verify_magic;
    if verify_magic(stored_magic, &magic) {
        Ok(Json(serde_json::json!({"success": true})))
    } else {
        Err(AppError::RepoPasswdMagicRequired)
    }
}

/// Extract the `password` field from a POST body (JSON, form-urlencoded, or
/// multipart/form-data).
fn parse_password_field(bytes: &[u8]) -> Result<String, AppError> {
    // Try JSON body
    if let Ok(map) = serde_json::from_slice::<HashMap<String, String>>(bytes)
        && let Some(pw) = map.get("password")
    {
        return Ok(pw.clone());
    }

    // Try form-urlencoded body
    if let Ok(map) = serde_urlencoded::from_bytes::<HashMap<String, String>>(bytes)
        && let Some(pw) = map.get("password")
    {
        return Ok(pw.clone());
    }

    // Fallback: multipart/form-data scan
    if let Some(pw) = extract_multipart_field(bytes, "password") {
        return Ok(pw);
    }

    Err(AppError::BadRequest("password required".into()))
}

/// Extract the `magic` field from a POST body (JSON, form-urlencoded, or
/// multipart/form-data).
fn parse_magic_field(bytes: &[u8]) -> Result<String, AppError> {
    // Try JSON body
    if let Ok(map) = serde_json::from_slice::<HashMap<String, String>>(bytes)
        && let Some(m) = map.get("magic")
    {
        return Ok(m.clone());
    }

    // Try form-urlencoded body
    if let Ok(map) = serde_urlencoded::from_bytes::<HashMap<String, String>>(bytes)
        && let Some(m) = map.get("magic")
    {
        return Ok(m.clone());
    }

    // Fallback: multipart/form-data scan
    if let Some(m) = extract_multipart_field(bytes, "magic") {
        return Ok(m);
    }

    Err(AppError::BadRequest("magic required".into()))
}

/// Extract a named field from a multipart/form-data body by scanning the
/// raw body for `name="<field_name>"` and returning the value that follows
/// the header-terminating `\r\n\r\n` boundary.
pub(crate) fn extract_multipart_field(bytes: &[u8], field_name: &str) -> Option<String> {
    let body_str = String::from_utf8_lossy(bytes);
    let pattern = format!("name=\"{}\"", field_name);
    let rest = body_str.split(&pattern).nth(1)?;
    // The value follows after the part headers which end with \r\n\r\n
    let val_block = rest.split("\r\n\r\n").nth(1)?;
    let value = val_block.split("\r\n").next().unwrap_or("").trim();
    if value.is_empty() {
        None
    } else {
        Some(value.to_string())
    }
}

/// Extract `repo_name` from POST body bytes, probing JSON, form-urlencoded,
/// then multipart/form-data in order.
fn parse_repo_name(bytes: &[u8]) -> Result<String, AppError> {
    // Try JSON body
    if let Ok(map) = serde_json::from_slice::<HashMap<String, String>>(bytes)
        && let Some(name) = map.get("repo_name")
    {
        return Ok(name.clone());
    }

    // Try form-urlencoded body
    if let Ok(map) = serde_urlencoded::from_bytes::<HashMap<String, String>>(bytes)
        && let Some(name) = map.get("repo_name")
    {
        return Ok(name.clone());
    }

    // Fallback: multipart/form-data – scan for `name="repo_name"` in the
    // raw body string.  The Android client sends the field as a single
    // multipart part; the value sits after the headers block (\r\n\r\n).
    let body_str = String::from_utf8_lossy(bytes);
    let pattern = "name=\"repo_name\"";
    if let Some(rest) = body_str.split(pattern).nth(1) {
        // Skip past \r\n\r\n that terminates the part headers
        if let Some(val_block) = rest.split("\r\n\r\n").nth(1) {
            let value = val_block.split("\r\n").next().unwrap_or("").trim();
            if !value.is_empty() {
                return Ok(value.to_string());
            }
        }
    }

    Err(AppError::BadRequest("repo_name required".into()))
}

pub async fn delete_repo(
    auth: AuthUser,
    State(state): State<Arc<AppState>>,
    Path(repo_id): Path<String>,
) -> Result<Json<serde_json::Value>, AppError> {
    let db = state.db.as_ref();

    // Load repo
    let r = repo::Entity::find_by_id(&repo_id)
        .one(db)
        .await?
        .ok_or_else(|| AppError::NotFound("repo not found".into()))?;

    // Only owner can delete
    if r.owner_id != auth.user_id {
        return Err(AppError::Forbidden);
    }

    // --- REPO TRASH: Record deleted repo before cascade-delete ---
    if let Err(e) = crate::storage::trash::TrashService::add_deleted_repo(
        db,
        &repo_id,
        &r.name,
        r.head_commit_id.as_deref(),
        r.owner_id,
        r.size,
    )
    .await
    {
        tracing::warn!("Failed to record deleted repo in trash: {e}");
    }
    // --- END REPO TRASH ---

    // Log repo deletion activity BEFORE deleting the repo (FK constraint
    // prevents inserting activity with a non-existent repo_id).
    crate::activity_log::log_activity(
        db,
        &repo_id,
        "delete",
        "repo",
        "/",
        auth.user_id,
        None,
        None,
        None,
        None,
        None,
    )
    .await;

    // Cascade-delete related records
    repo_member::Entity::delete_many()
        .filter(repo_member::Column::RepoId.eq(&repo_id))
        .exec(db)
        .await?;

    sync_token::Entity::delete_many()
        .filter(sync_token::Column::RepoId.eq(&repo_id))
        .exec(db)
        .await?;

    // Delete the repo itself
    repo::Entity::delete_by_id(&repo_id).exec(db).await?;

    // Return a JSON string "success" (not an object) so the Android
    // client's SupportResponseConverter can parse it as String.
    Ok(Json(serde_json::Value::String("success".to_string())))
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
///
/// Uses the request's `Host` header (when available) so the returned URL is
/// reachable from the client's perspective, even when the server listens on
/// `0.0.0.0` or `127.0.0.1` (which would otherwise produce an unreachable URL
/// for remote or emulator-based clients).
fn build_op_url(state: &AppState, op: &str, token: &str, host_header: Option<&str>) -> String {
    let (host, port) = if let Some(h) = host_header {
        // Use the Host header from the incoming request.
        // Host may be "host:port" or just "host".
        if let Some((h, p)) = h.split_once(':') {
            (h.to_string(), p.to_string())
        } else {
            (h.to_string(), state.config.server.port.to_string())
        }
    } else if state.config.server.addr == "0.0.0.0"
        || state.config.server.addr == "::"
        || state.config.server.addr == "127.0.0.1"
    {
        (
            "127.0.0.1".to_string(),
            state.config.server.port.to_string(),
        )
    } else {
        (
            state.config.server.addr.clone(),
            state.config.server.port.to_string(),
        )
    };
    format!("http://{}:{}/{}/{}", host, port, op, token)
}

pub async fn get_upload_link(
    auth: AuthUser,
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
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
    let host_header = headers.get("host").and_then(|v| v.to_str().ok());
    let mut url = build_op_url(&state, op, &token, host_header);

    // When from=api, optionally append ?replace=1
    if !is_web && query.replace.as_deref() == Some("1") {
        url.push_str("?replace=1");
    }

    Ok(Json(url))
}

pub async fn get_update_link(
    auth: AuthUser,
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
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
    let host_header = headers.get("host").and_then(|v| v.to_str().ok());
    let url = build_op_url(&state, op, &token, host_header);

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
                Some(r) => {
                    let encrypted = r.encrypted != 0;
                    Ok(Json(RepoInfo {
                        id: r.id.clone(),
                        name: r.name.clone(),
                        desc: r.description.clone(),
                        owner: auth.email,
                        encrypted,
                        enc_version: if encrypted {
                            Some(r.enc_version as i32)
                        } else {
                            None
                        },
                        size: r.size,
                        mtime: r.updated_at,
                        permission: m.permission.clone(),
                        head_commit_id: r.head_commit_id.clone(),
                        type_: if r.owner_id == auth.user_id {
                            "repo".to_string()
                        } else {
                            "srepo".to_string()
                        },
                        virtual_: false,
                        root: None,
                        salt: if encrypted && r.enc_version >= 3 {
                            Some(r.salt.clone())
                        } else {
                            None
                        },
                        magic: if encrypted { r.magic.clone() } else { None },
                        random_key: if encrypted {
                            r.random_key.clone()
                        } else {
                            None
                        },
                        repo_version: r.repo_version,
                        lib_need_decrypt: if encrypted { Some(true) } else { None },
                        repo_id_dup: None,
                        repo_name_dup: None,
                        token: None,
                        email: None,
                    }))
                }
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
            let encrypted = r.encrypted != 0;
            return Ok((
                StatusCode::OK,
                Json(RepoInfo {
                    id: r.id.clone(),
                    name: r.name.clone(),
                    desc: r.description.clone(),
                    owner: auth.email,
                    encrypted,
                    enc_version: if encrypted {
                        Some(r.enc_version as i32)
                    } else {
                        None
                    },
                    size: r.size,
                    mtime: r.updated_at,
                    permission: r.permission.clone(),
                    head_commit_id: r.head_commit_id.clone(),
                    type_: "repo".to_string(),
                    virtual_: false,
                    root: None,
                    salt: if encrypted && r.enc_version >= 3 {
                        Some(r.salt.clone())
                    } else {
                        None
                    },
                    magic: if encrypted { r.magic.clone() } else { None },
                    random_key: if encrypted {
                        r.random_key.clone()
                    } else {
                        None
                    },
                    repo_version: r.repo_version,
                    lib_need_decrypt: if encrypted { Some(true) } else { None },
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
            enc_version: None,
            size: 0,
            mtime: now,
            permission: "rw".to_string(),
            head_commit_id: None,
            type_: "repo".to_string(),
            virtual_: false,
            root: None,
            salt: None,
            magic: None,
            random_key: None,
            repo_version: 1,
            lib_need_decrypt: None,
            repo_id_dup: Some(repo_id),
            repo_name_dup: Some("我的资料库".to_string()),
            token: Some(token_value),
            email: Some(auth.email),
        }),
    ))
}
