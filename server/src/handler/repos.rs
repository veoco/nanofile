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
use crate::middleware::auth::AuthUser;
use crate::service::repo::password::PasswordService;
use crate::service::repo::service;
use base::error::AppError;
use infra::common::util::extract_multipart_field;
// Re-export response types for api module re-exports
pub use crate::service::repo::service::{
    DownloadInfoResponse, RepoInfo, V21RepoInfo, V21RepoListResponse,
};

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
    let repos = service::RepoService::list_repos(
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

    let (repo_info, _token) = service::RepoService::create_repo(
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
    let repo_info = service::RepoService::get_repo(
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

    let repo_name = parse_body_field(&bytes, "repo_name", "repo_name required")?;

    service::RepoService::rename_repo(
        state.db.as_ref(),
        &state.repos,
        &repo_id,
        auth.user_id,
        &repo_name,
    )
    .await?;

    Ok(Json(serde_json::Value::String("success".to_string())))
}

#[derive(Deserialize)]
struct UpdateRepoRequest {
    repo_name: Option<String>,
    description: Option<String>,
}

/// `POST /api2/repos/{repo_id}/?op=update`
///
/// Updates repo name and/or description. Only the owner can update.
pub async fn update_repo(
    auth: AuthUser,
    State(state): State<Arc<AppState>>,
    Path(repo_id): Path<String>,
    req: axum::http::Request<Body>,
) -> Result<Json<serde_json::Value>, AppError> {
    let (_parts, body) = req.into_parts();
    let bytes = axum::body::to_bytes(body, usize::MAX)
        .await
        .map_err(|e| AppError::Internal(e.to_string()))?;

    let update: UpdateRepoRequest = serde_json::from_slice(&bytes)
        .map_err(|e| AppError::BadRequest(format!("invalid JSON: {e}")))?;

    service::RepoService::update_repo(
        state.db.as_ref(),
        &state.repos,
        &repo_id,
        auth.user_id,
        update.repo_name,
        update.description,
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
        Some("update") => update_repo(auth, state, Path(repo_id), req).await,
        Some("setpassword") => set_repo_password_v2(auth, state, Path(repo_id), req).await,
        Some("checkpassword") => check_repo_password_v2(auth, state, Path(repo_id), req).await,
        _ => Err(AppError::BadRequest(
            "invalid operation; use rename, update, setpassword, or checkpassword".into(),
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

    let password = parse_body_field(&bytes, "password", "password required")?;

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
    auth: AuthUser,
    State(state): State<Arc<AppState>>,
    Path(repo_id): Path<String>,
    req: axum::http::Request<Body>,
) -> Result<Json<serde_json::Value>, AppError> {
    // Check user has access to this repo (matching seahub's check_folder_permission).
    crate::domain::permission::check_repo_read_permission(
        state.db.as_ref(),
        &repo_id,
        auth.user_id,
    )
    .await?;

    let (_parts, body) = req.into_parts();
    let bytes = axum::body::to_bytes(body, usize::MAX)
        .await
        .map_err(|e| AppError::Internal(e.to_string()))?;

    let magic = parse_body_field(&bytes, "magic", "magic required")?;

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

    use infra::crypto::verify::verify_magic;
    if verify_magic(stored_magic, &magic) {
        Ok(Json(serde_json::json!({"success": true})))
    } else {
        Err(AppError::RepoPasswdMagicRequired)
    }
}

/// Extract a field from a POST body using the standard triple probe
/// (JSON → form-urlencoded → multipart/form-data).
fn parse_body_field(bytes: &[u8], field: &str, error_msg: &str) -> Result<String, AppError> {
    infra::common::util::extract_body_field(bytes, field)
        .ok_or_else(|| AppError::BadRequest(error_msg.into()))
}

pub async fn delete_repo(
    auth: AuthUser,
    State(state): State<Arc<AppState>>,
    Path(repo_id): Path<String>,
) -> Result<Json<serde_json::Value>, AppError> {
    service::RepoService::delete_repo(state.db.as_ref(), &state.repos, &repo_id, auth.user_id)
        .await?;

    Ok(Json(serde_json::Value::String("success".to_string())))
}

pub async fn download_info(
    auth: AuthUser,
    State(state): State<Arc<AppState>>,
    Path(repo_id): Path<String>,
) -> Result<Json<DownloadInfoResponse>, AppError> {
    let info = service::RepoService::download_info(
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

    let url = service::RepoService::get_upload_link(
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

    let url = service::RepoService::get_update_link(
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

    let result =
        service::RepoService::repo_tokens(state.db.as_ref(), &state.repos, &repo_ids, auth.user_id)
            .await?;

    Ok(Json(result))
}

/// GET /api/v2.1/repos/
pub async fn list_repos_v21(
    auth: AuthUser,
    State(state): State<Arc<AppState>>,
) -> Result<Json<V21RepoListResponse>, AppError> {
    let response = service::RepoService::list_repos_v21(
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
    let repo_info = service::RepoService::get_repo_v21(
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
    service::RepoService::delete_repo(state.db.as_ref(), &state.repos, &repo_id, auth.user_id)
        .await?;

    Ok(Json(serde_json::Value::String("success".to_string())))
}
