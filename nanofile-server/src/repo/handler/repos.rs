use axum::{
    Json, Router,
    body::{Body, Bytes},
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
};
use serde::Deserialize;
use std::collections::HashMap;
use std::sync::Arc;

use crate::AppState;
use crate::auth::middleware::AuthUser;
use crate::common::util::extract_multipart_field;
use crate::error::AppError;
use crate::repo::service::password_service::PasswordService;
use crate::repo::service::repo_service;
// Re-export response types for api module re-exports
pub use crate::repo::service::repo_service::{DownloadInfoResponse, RepoInfo, V21RepoInfo, V21RepoListResponse};

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
    let repos = repo_service::RepoService::list_repos(
        state.db.as_ref(),
        &state.repos,
        auth.user_id,
        &auth.email,
    )
    .await?;
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

    let (repo_info, _token) = repo_service::RepoService::create_repo(
        state.db.as_ref(),
        &state.repos,
        auth.user_id,
        &auth.email,
        &repo_req.name,
        &repo_req.desc.unwrap_or_default(),
        repo_req.repo_id,
        repo_req.encrypted.unwrap_or(0),
        repo_req.enc_version.unwrap_or(0),
        repo_req.magic.clone(),
        repo_req.random_key.clone(),
    )
    .await?;

    Ok((StatusCode::CREATED, Json(repo_info)))
}

pub async fn get_repo(
    auth: AuthUser,
    State(state): State<Arc<AppState>>,
    Path(repo_id): Path<String>,
) -> Result<Json<RepoInfo>, AppError> {
    let repo_info = repo_service::RepoService::get_repo(
        state.db.as_ref(),
        &state.repos,
        &repo_id,
        auth.user_id,
        &auth.email,
    )
    .await?;
    Ok(Json(repo_info))
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

    // Parse repo_name from body, trying JSON, form-urlencoded, then multipart
    let (_parts, body) = req.into_parts();
    let bytes = axum::body::to_bytes(body, usize::MAX)
        .await
        .map_err(|e| AppError::Internal(e.to_string()))?;

    let repo_name = parse_repo_name(&bytes)?;

    repo_service::RepoService::rename_repo(
        state.db.as_ref(),
        &state.repos,
        &repo_id,
        auth.user_id,
        &repo_name,
    )
    .await?;

    Ok(Json(serde_json::Value::String("success".to_string())))
}

/// `POST /api2/repos/{repo_id}/`
///
/// Dispatches to the appropriate handler based on the `op` query parameter.
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

    let password = parse_password_field(&bytes)?;

    PasswordService::set_password(
        &state.password_manager,
        &state.repos,
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

    let magic = parse_magic_field(&bytes)?;

    // Load the repo
    let repo_model = state
        .repos
        .repo
        .find_by_id(&repo_id)
        .await?
        .ok_or_else(|| AppError::NotFound("repo not found".into()))?;

    if repo_model.encrypted == 0 {
        return Err(AppError::BadRequest("repo is not encrypted".into()));
    }

    let stored_magic = repo_model
        .magic
        .as_deref()
        .ok_or_else(|| AppError::BadRequest("repo has no stored magic".into()))?;

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
    if let Ok(map) = serde_json::from_slice::<HashMap<String, String>>(bytes)
        && let Some(pw) = map.get("password")
    {
        return Ok(pw.clone());
    }

    if let Ok(map) = serde_urlencoded::from_bytes::<HashMap<String, String>>(bytes)
        && let Some(pw) = map.get("password")
    {
        return Ok(pw.clone());
    }

    if let Some(pw) = extract_multipart_field(bytes, "password") {
        return Ok(pw);
    }

    Err(AppError::BadRequest("password required".into()))
}

/// Extract the `magic` field from a POST body (JSON, form-urlencoded, or
/// multipart/form-data).
fn parse_magic_field(bytes: &[u8]) -> Result<String, AppError> {
    if let Ok(map) = serde_json::from_slice::<HashMap<String, String>>(bytes)
        && let Some(m) = map.get("magic")
    {
        return Ok(m.clone());
    }

    if let Ok(map) = serde_urlencoded::from_bytes::<HashMap<String, String>>(bytes)
        && let Some(m) = map.get("magic")
    {
        return Ok(m.clone());
    }

    if let Some(m) = extract_multipart_field(bytes, "magic") {
        return Ok(m);
    }

    Err(AppError::BadRequest("magic required".into()))
}

/// Extract `repo_name` from POST body bytes, probing JSON, form-urlencoded,
/// then multipart/form-data in order.
fn parse_repo_name(bytes: &[u8]) -> Result<String, AppError> {
    if let Ok(map) = serde_json::from_slice::<HashMap<String, String>>(bytes)
        && let Some(name) = map.get("repo_name")
    {
        return Ok(name.clone());
    }

    if let Ok(map) = serde_urlencoded::from_bytes::<HashMap<String, String>>(bytes)
        && let Some(name) = map.get("repo_name")
    {
        return Ok(name.clone());
    }

    let body_str = String::from_utf8_lossy(bytes);
    let pattern = "name=\"repo_name\"";
    if let Some(rest) = body_str.split(pattern).nth(1) {
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
    repo_service::RepoService::delete_repo(
        state.db.as_ref(),
        &state.repos,
        &repo_id,
        auth.user_id,
    )
    .await?;

    Ok(Json(serde_json::Value::String("success".to_string())))
}

pub async fn download_info(
    auth: AuthUser,
    State(state): State<Arc<AppState>>,
    Path(repo_id): Path<String>,
) -> Result<Json<DownloadInfoResponse>, AppError> {
    let info = repo_service::RepoService::download_info(
        state.db.as_ref(),
        &state.repos,
        &repo_id,
        auth.user_id,
    )
    .await?;
    Ok(Json(info))
}

#[derive(Deserialize)]
pub struct LinkQuery {
    pub p: Option<String>,
    pub from: Option<String>,
    pub replace: Option<String>,
}

pub async fn get_upload_link(
    auth: AuthUser,
    State(state): State<Arc<AppState>>,
    _headers: HeaderMap,
    Path(repo_id): Path<String>,
    Query(query): Query<LinkQuery>,
) -> Result<Json<String>, AppError> {
    let parent_dir = query.p.as_deref().unwrap_or("/");

    let url = repo_service::RepoService::get_upload_link(
        state.db.as_ref(),
        &state.repos,
        &state.token_manager,
        &state.config.server.site_url,
        &repo_id,
        auth.user_id,
        &auth.email,
        parent_dir,
        query.from.as_deref(),
        query.replace.as_deref(),
    )
    .await?;

    Ok(Json(url))
}

pub async fn get_update_link(
    auth: AuthUser,
    State(state): State<Arc<AppState>>,
    _headers: HeaderMap,
    Path(repo_id): Path<String>,
    Query(query): Query<LinkQuery>,
) -> Result<Json<String>, AppError> {
    let parent_dir = query.p.as_deref().unwrap_or("/");

    let url = repo_service::RepoService::get_update_link(
        state.db.as_ref(),
        &state.repos,
        &state.token_manager,
        &state.config.server.site_url,
        &repo_id,
        auth.user_id,
        &auth.email,
        parent_dir,
        query.from.as_deref(),
    )
    .await?;

    Ok(Json(url))
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

    let result = repo_service::RepoService::repo_tokens(
        state.db.as_ref(),
        &state.repos,
        &repo_ids,
        auth.user_id,
    )
    .await?;

    Ok(Json(result))
}

/// `GET /api2/default-repo/`
pub async fn get_default_repo(
    auth: AuthUser,
    State(state): State<Arc<AppState>>,
) -> Result<Json<RepoInfo>, AppError> {
    let repo_info = repo_service::RepoService::get_default_repo(
        state.db.as_ref(),
        &state.repos,
        auth.user_id,
        &auth.email,
    )
    .await?;
    Ok(Json(repo_info))
}

/// `POST /api2/default-repo/`
pub async fn create_default_repo(
    auth: AuthUser,
    State(state): State<Arc<AppState>>,
) -> Result<(StatusCode, Json<RepoInfo>), AppError> {
    let (repo_info, _token) = repo_service::RepoService::create_default_repo(
        state.db.as_ref(),
        &state.repos,
        auth.user_id,
        &auth.email,
    )
    .await?;

    // Check if it was a new creation or existing
    let is_new = repo_info.repo_id_dup.is_some();
    let status = if is_new {
        StatusCode::CREATED
    } else {
        StatusCode::OK
    };

    Ok((status, Json(repo_info)))
}

/// GET /api/v2.1/repos/
pub async fn list_repos_v21(
    auth: AuthUser,
    State(state): State<Arc<AppState>>,
) -> Result<Json<V21RepoListResponse>, AppError> {
    let response = repo_service::RepoService::list_repos_v21(
        state.db.as_ref(),
        &state.repos,
        auth.user_id,
        &auth.email,
    )
    .await?;
    Ok(Json(response))
}

/// GET /api/v2.1/repos/{repo_id}/
pub async fn get_repo_v21(
    auth: AuthUser,
    State(state): State<Arc<AppState>>,
    Path(repo_id): Path<String>,
) -> Result<Json<V21RepoInfo>, AppError> {
    let repo_info = repo_service::RepoService::get_repo_v21(
        state.db.as_ref(),
        &state.repos,
        &repo_id,
        auth.user_id,
        &auth.email,
    )
    .await?;
    Ok(Json(repo_info))
}

pub async fn delete_repo_v21(
    auth: AuthUser,
    State(state): State<Arc<AppState>>,
    Path(repo_id): Path<String>,
) -> Result<Json<serde_json::Value>, AppError> {
    repo_service::RepoService::delete_repo(
        state.db.as_ref(),
        &state.repos,
        &repo_id,
        auth.user_id,
    )
    .await?;

    Ok(Json(serde_json::Value::String("success".to_string())))
}
