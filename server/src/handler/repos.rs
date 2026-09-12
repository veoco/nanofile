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
use crate::handler::{MAX_SMALL_BODY_BYTES, ok_json, read_body_limited};
use crate::middleware::auth::AuthUser;
use crate::service::repo::password::PasswordService;
use crate::service::repo::service;
use base::error::AppError;
use infra::common::util::extract_multipart_field;
// Re-export response types for api module re-exports
pub use crate::service::repo::service::{
    DownloadInfoResponse, RepoInfo, V21RepoInfo, V21RepoListResponse,
};

/// Accept an integer that may arrive as a JSON number or as a string.
///
/// The official clients send this endpoint as
/// `application/x-www-form-urlencoded` (and, for the Android client,
/// `multipart/form-data`), where every value is a string. The desktop client in
/// particular sends `enc_version` as `QString::number(enc_version)`, i.e.
/// `"4"` — deserializing that into `i32` used to fail the whole request with a
/// 500 for every encrypted-library creation.
fn de_int_or_string<'de, D>(deserializer: D) -> Result<Option<i32>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    #[derive(serde::Deserialize)]
    #[serde(untagged)]
    enum IntOrString {
        Int(i64),
        Str(String),
    }

    let value = Option::<IntOrString>::deserialize(deserializer)?;
    match value {
        None => Ok(None),
        Some(IntOrString::Int(v)) => i32::try_from(v)
            .map(Some)
            .map_err(|_| serde::de::Error::custom(format!("value out of range: {v}"))),
        Some(IntOrString::Str(s)) => {
            let s = s.trim();
            if s.is_empty() {
                return Ok(None);
            }
            s.parse::<i32>().map(Some).map_err(serde::de::Error::custom)
        }
    }
}

#[derive(Deserialize)]
pub struct CreateRepoRequest {
    pub name: String,
    pub desc: Option<String>,
    pub repo_id: Option<String>,
    #[serde(default, deserialize_with = "de_int_or_string")]
    pub encrypted: Option<i32>,
    #[serde(default, deserialize_with = "de_int_or_string")]
    pub enc_version: Option<i32>,
    pub magic: Option<String>,
    pub random_key: Option<String>,
    /// Per-library random salt for `enc_version` 4 libraries (empty/ignored for
    /// v2, which uses a fixed salt). The desktop client sends it for v3/v4.
    pub salt: Option<String>,
    /// Only used by clients that compute the magic themselves; nanofile derives
    /// from `magic`/`random_key`, so these are accepted and ignored.
    pub pwd_hash_algo: Option<String>,
    pub pwd_hash_params: Option<String>,
    pub pwd_hash: Option<String>,
    /// Sent by clients that let the server hash the password instead of
    /// pre-computing `magic`/`random_key`.
    pub passwd: Option<String>,
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
    let repos = service::RepoService::list_repos(&state.repos, auth.user_id, &auth.email).await?;
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
    //
    // Both parse failures must be client errors (400). A plain `?` on the JSON
    // branch used to go through `From<serde_json::Error>` → `Internal`, which
    // turned the desktop client's string-typed `enc_version` into a 500 for
    // every encrypted-library creation.
    //
    // The branch is chosen by content type rather than by trying each parser in
    // turn: a form body whose `enc_version` is not a number must report *that*,
    // not fall through to the multipart parser and claim `name required`.
    let content_type = headers
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default();
    let repo_req: CreateRepoRequest = if content_type.contains("json") {
        serde_json::from_slice(&bytes)
            .map_err(|e| AppError::BadRequest(format!("invalid JSON body: {e}")))?
    } else if content_type.contains("multipart/form-data") {
        let mut req = CreateRepoRequest {
            name: String::new(),
            desc: None,
            repo_id: None,
            encrypted: None,
            enc_version: None,
            magic: None,
            random_key: None,
            salt: None,
            pwd_hash_algo: None,
            pwd_hash_params: None,
            pwd_hash: None,
            passwd: None,
        };
        req.name = extract_multipart_field(&bytes, "name")
            .ok_or_else(|| AppError::BadRequest("name required".into()))?;
        req.desc = extract_multipart_field(&bytes, "desc");
        req
    } else {
        // Default (and explicit `application/x-www-form-urlencoded`): the
        // desktop client's format.
        serde_urlencoded::from_bytes::<CreateRepoRequest>(&bytes)
            .map_err(|e| AppError::BadRequest(format!("invalid form body: {e}")))?
    };

    let (repo_info, _token) = service::RepoService::create_repo(
        state.db.as_ref(),
        &state.repos,
        auth.user_id,
        &auth.email,
        &repo_req.name,
        &repo_req.desc.clone().unwrap_or_default(),
        repo_req.repo_id.clone(),
        repo_req.encrypted.unwrap_or(0),
        repo_req.enc_version.unwrap_or(0),
        repo_req.magic.clone(),
        repo_req.random_key.clone(),
        repo_req.salt.clone(),
        state.config.auth.sync_token_ttl_days,
    )
    .await?;

    state.left_panel_cache.clear_all();

    Ok((StatusCode::CREATED, Json(repo_info)))
}

pub async fn get_repo(
    auth: AuthUser,
    State(state): State<Arc<AppState>>,
    Path(repo_id): Path<String>,
) -> Result<Json<RepoInfo>, AppError> {
    let repo_info =
        service::RepoService::get_repo(&state.repos, &repo_id, auth.user_id, &auth.email).await?;
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
    let bytes = read_body_limited(body, MAX_SMALL_BODY_BYTES).await?;

    let repo_name = parse_body_field(&bytes, "repo_name", "repo_name required")?;

    service::RepoService::rename_repo(
        state.db.as_ref(),
        &state.repos,
        &repo_id,
        auth.user_id,
        &repo_name,
    )
    .await?;

    state.left_panel_cache.clear_all();

    Ok(Json(serde_json::Value::String("success".to_string())))
}

#[derive(Deserialize)]
struct UpdateRepoRequest {
    repo_name: Option<String>,
    description: Option<String>,
    history_limit: Option<i32>,
    history_ttl_days: Option<i32>,
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
    let bytes = read_body_limited(body, MAX_SMALL_BODY_BYTES).await?;

    let update: UpdateRepoRequest = serde_json::from_slice(&bytes)
        .map_err(|e| AppError::BadRequest(format!("invalid JSON: {e}")))?;

    service::RepoService::update_repo(
        state.db.as_ref(),
        &state.repos,
        &repo_id,
        auth.user_id,
        update.repo_name,
        update.description,
        update.history_limit,
        update.history_ttl_days,
    )
    .await?;

    state.left_panel_cache.clear_all();

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
    // Membership is required before the password is even looked at. This
    // endpoint verifies the supplied password against the stored `magic` and
    // returns a different status for a hit, so without this check it is a
    // password oracle for any authenticated non-member who knows the repo id.
    // The KDF is fixed by the wire protocol at a low iteration count, so the
    // per-(user, repo) limiter below is the practical brute-force control.
    crate::domain::permission::check_repo_read_permission(
        state.repos.member.as_ref(),
        &repo_id,
        auth.user_id,
    )
    .await?;

    if state
        .auth_limiters
        .is_repo_password_limited(auth.user_id, &repo_id)
    {
        return Err(AppError::TooManyRequests);
    }

    let (_parts, body) = req.into_parts();
    let bytes = read_body_limited(body, MAX_SMALL_BODY_BYTES).await?;

    let password = parse_body_field(&bytes, "password", "password required")?;

    if let Err(e) = PasswordService::set_password(
        &state.password_manager,
        &state.repos,
        &repo_id,
        auth.user_id,
        &password,
    )
    .await
    {
        // A wrong password is what the limiter counts; other failures (unknown
        // repo, not encrypted) are not guesses and should not consume budget.
        if matches!(e, base::error::AppError::RepoPasswdRequired) {
            state
                .auth_limiters
                .record_repo_password_failure(auth.user_id, &repo_id);
        }
        return Err(e);
    }

    // A correct password clears the budget: clients re-submit the cached
    // password repeatedly (Android's download worker does so per download), so
    // successes must never accumulate.
    state
        .auth_limiters
        .clear_repo_password_failures(auth.user_id, &repo_id);

    Ok(ok_json())
}

/// `POST /api2/repos/{repo_id}/?op=checkpassword`
///
/// Check if a password is valid for an encrypted repo (v2 API).
///
/// This endpoint verifies a caller-supplied `magic` (the password-equivalent
/// value the client derives locally), so it is a password oracle and is metered
/// with the same per-(user, repo) limiter as `?op=setpassword` and the v2.1
/// `set-password` endpoint.
pub async fn check_repo_password_v2(
    auth: AuthUser,
    State(state): State<Arc<AppState>>,
    Path(repo_id): Path<String>,
    req: axum::http::Request<Body>,
) -> Result<Json<serde_json::Value>, AppError> {
    // Check user has access to this repo (matching seahub's check_folder_permission).
    crate::domain::permission::check_repo_read_permission(
        state.repos.member.as_ref(),
        &repo_id,
        auth.user_id,
    )
    .await?;

    if state
        .auth_limiters
        .is_repo_password_limited(auth.user_id, &repo_id)
    {
        return Err(AppError::TooManyRequests);
    }

    let (_parts, body) = req.into_parts();
    let bytes = read_body_limited(body, MAX_SMALL_BODY_BYTES).await?;

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
        state
            .auth_limiters
            .clear_repo_password_failures(auth.user_id, &repo_id);
        Ok(ok_json())
    } else {
        // Only a wrong magic is a guess; "not encrypted"/"no magic" above are
        // configuration errors and must not consume the budget.
        state
            .auth_limiters
            .record_repo_password_failure(auth.user_id, &repo_id);
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

    state.left_panel_cache.clear_all();

    Ok(Json(serde_json::Value::String("success".to_string())))
}

pub async fn download_info(
    auth: AuthUser,
    State(state): State<Arc<AppState>>,
    Path(repo_id): Path<String>,
) -> Result<Json<DownloadInfoResponse>, AppError> {
    let info = service::RepoService::download_info(
        &state.repos,
        &repo_id,
        auth.user_id,
        state.config.auth.sync_token_ttl_days,
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

    let result = service::RepoService::repo_tokens(
        &state.repos,
        &repo_ids,
        auth.user_id,
        state.config.auth.sync_token_ttl_days,
    )
    .await?;

    Ok(Json(result))
}

/// GET /api/v2.1/repos/
pub async fn list_repos_v21(
    auth: AuthUser,
    State(state): State<Arc<AppState>>,
) -> Result<Json<V21RepoListResponse>, AppError> {
    let response =
        service::RepoService::list_repos_v21(&state.repos, auth.user_id, &auth.email).await?;
    Ok(Json(response))
}

/// GET /api/v2.1/repos/{repo_id}/
pub async fn get_repo_v21(
    auth: AuthUser,
    State(state): State<Arc<AppState>>,
    Path(repo_id): Path<String>,
) -> Result<Json<V21RepoInfo>, AppError> {
    let repo_info =
        service::RepoService::get_repo_v21(&state.repos, &repo_id, auth.user_id, &auth.email)
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

    state.left_panel_cache.clear_all();

    Ok(Json(serde_json::Value::String("success".to_string())))
}

/// `GET /api2/default-repo/` — the user's default (primary) library.
///
/// Returns `{"exists": true, "repo_id": <id>}` when the user has a library,
/// otherwise `{"exists": false}` (matching seahub's `DefaultRepoView`).
pub async fn get_default_repo(
    auth: AuthUser,
    State(state): State<Arc<AppState>>,
) -> Result<Json<serde_json::Value>, AppError> {
    let default_id = service::RepoService::default_repo_id(&state.repos, auth.user_id).await?;
    match default_id {
        Some(id) => Ok(Json(serde_json::json!({"exists": true, "repo_id": id}))),
        None => Ok(Json(serde_json::json!({"exists": false}))),
    }
}

/// `POST /api2/default-repo/` — find-or-create the default library.
///
/// Returns `{"repo_id": <id>}`. Creates a "My Library" repo when the user has
/// no owned repo yet (matching seahub's default library name).
pub async fn create_default_repo(
    auth: AuthUser,
    State(state): State<Arc<AppState>>,
) -> Result<Json<serde_json::Value>, AppError> {
    let default_id = service::RepoService::default_repo_id(&state.repos, auth.user_id).await?;
    let id = match default_id {
        Some(id) => id,
        None => {
            let (repo_info, _token) = service::RepoService::create_repo(
                state.db.as_ref(),
                &state.repos,
                auth.user_id,
                &auth.email,
                "My Library",
                "My Library",
                None,
                0,
                0,
                None,
                None,
                None,
                state.config.auth.sync_token_ttl_days,
            )
            .await?;
            repo_info.id
        }
    };
    state.left_panel_cache.clear_all();
    Ok(Json(serde_json::json!({"repo_id": id})))
}
