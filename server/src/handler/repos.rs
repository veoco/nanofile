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
pub use crate::service::repo::service::{
    DownloadInfoResponse, RepoInfo, V21RepoInfo, V21RepoListResponse,
};
use base::error::AppError;
use infra::common::util::extract_multipart_field;

/// Accept an integer that may arrive as a JSON number or as a string.
///
/// Official clients send form/multipart bodies where every value is a string
/// (the desktop client sends `enc_version` as `"4"` via `QString::number`), so a
/// strict `i32` would reject the request.
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
    /// Per-library random salt for `enc_version` 4 libraries; ignored for v2 (fixed salt).
    pub salt: Option<String>,
    /// `pwd_hash` verifier triple a client pre-computed when `/api2/server-info/`
    /// advertised an algorithm; when `pwd_hash_algo` is present the client omits
    /// `magic` and the library is identified by this triple from then on.
    pub pwd_hash_algo: Option<String>,
    pub pwd_hash_params: Option<String>,
    pub pwd_hash: Option<String>,
    /// Sent by clients that let the server hash the password instead of `magic`.
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
    let mut repos =
        service::RepoService::list_repos(&state.repos, auth.user_id, &auth.email).await?;
    // A key bound to specific libraries must not enumerate the others.
    let scope = auth.repo_scope();
    repos.retain(|repo| scope.allows(&repo.id));
    Ok(Json(repos))
}

pub async fn create_repo(
    auth: AuthUser,
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    bytes: Bytes,
) -> Result<(StatusCode, Json<RepoInfo>), AppError> {
    // Support JSON (web), form-encoded (desktop) and multipart/form-data (Android).
    //
    // Both parse failures must be 400; the branch is chosen by content type so a
    // bad form field is reported instead of falling through to the multipart parser.
    let content_type = headers
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default();
    let repo_req: CreateRepoRequest = if content_type.contains("json") {
        serde_json::from_slice(&bytes)
            .map_err(|e| AppError::BadRequest(format!("invalid JSON body: {e}")))?
    } else if content_type.contains("multipart/form-data") {
        // The Android client sends multipart/form-data with `name`, optional
        // `desc` and a bare `passwd` for an encrypted library; every field the
        // other encodings accept must be read here too.
        let field = |name: &str| extract_multipart_field(&bytes, name);
        CreateRepoRequest {
            name: field("name").ok_or_else(|| AppError::BadRequest("name required".into()))?,
            desc: field("desc"),
            repo_id: field("repo_id"),
            encrypted: field("encrypted").and_then(|v| v.trim().parse().ok()),
            enc_version: field("enc_version").and_then(|v| v.trim().parse().ok()),
            magic: field("magic"),
            random_key: field("random_key"),
            salt: field("salt"),
            pwd_hash_algo: field("pwd_hash_algo"),
            pwd_hash_params: field("pwd_hash_params"),
            pwd_hash: field("pwd_hash"),
            passwd: field("passwd"),
        }
    } else {
        // Default (and explicit `application/x-www-form-urlencoded`): desktop client.
        serde_urlencoded::from_bytes::<CreateRepoRequest>(&bytes)
            .map_err(|e| AppError::BadRequest(format!("invalid form body: {e}")))?
    };

    let peer = auth.device.as_ref().map(|device| device.peer());
    let repo_info = service::RepoService::create_repo(
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
        service::PwdHash::from_request(
            repo_req.pwd_hash.clone(),
            repo_req.pwd_hash_algo.clone(),
            repo_req.pwd_hash_params.clone(),
        ),
        repo_req.passwd.as_deref(),
        &service::EncryptedLibraryPolicy::from_config(&state.config().server),
        peer.as_ref(),
        state.config().auth.sync_token_ttl_days,
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
/// `repo_name` arrives as JSON, form-urlencoded or multipart (`name="repo_name"`).
pub async fn rename_repo(
    auth: AuthUser,
    State(state): State<Arc<AppState>>,
    Path(repo_id): Path<String>,
    Query(params): Query<HashMap<String, String>>,
    req: axum::http::Request<Body>,
) -> Result<Json<serde_json::Value>, AppError> {
    match params.get("op").map(|s| s.as_str()) {
        Some("rename") => {}
        _ => return Err(AppError::BadRequest("invalid operation".into())),
    }

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

/// `POST /api2/repos/{repo_id}/?op=update` — update name/description (owner only).
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

/// `POST /api2/repos/{repo_id}/` — dispatch on the `op` query parameter.
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
        // The official desktop client sets a library password with a bare
        // `POST /api2/repos/{id}/` carrying form field `password` and no `op`
        // (`requests.cpp:701-711`); route that to the same service as `?op=setpassword`.
        _ => set_repo_password_bare(auth, state, Path(repo_id), req).await,
    }
}

/// A wrong library password, in the shape each client recognises.
///
/// The desktop Qt client branches on the 400 status (`set-repo-password-dialog.cpp:57-60`)
/// while iOS matches `error_msg == "Incorrect password"` (`SeafRepos.m:250-256`);
/// both are satisfied by this. The v2.1 endpoint keeps its 440 because Android
/// maps that status (`ExceptionUtils.java:209-217`).
fn incorrect_password() -> AppError {
    AppError::BadRequest("Incorrect password".into())
}

/// `POST /api2/repos/{repo_id}/` — the desktop client's password-set form.
///
/// Differs from [`set_repo_password_v2`] only in returning "invalid operation"
/// when no `password` field is present.
pub async fn set_repo_password_bare(
    auth: AuthUser,
    State(state): State<Arc<AppState>>,
    Path(repo_id): Path<String>,
    req: axum::http::Request<Body>,
) -> Result<Json<serde_json::Value>, AppError> {
    let (_parts, body) = req.into_parts();
    let bytes = read_body_limited(body, MAX_SMALL_BODY_BYTES).await?;

    if infra::common::util::extract_body_field(&bytes, "password").is_none() {
        return Err(AppError::BadRequest(
            "invalid operation; use rename, update, setpassword, or checkpassword".into(),
        ));
    }

    set_repo_password_with_body(&state, auth.user_id, &repo_id, &bytes).await
}

/// `POST /api2/repos/{repo_id}/?op=setpassword` — set an encrypted repo's password
/// (v2 API). iOS calls this form and reads `error_msg` on failure.
pub async fn set_repo_password_v2(
    auth: AuthUser,
    State(state): State<Arc<AppState>>,
    Path(repo_id): Path<String>,
    req: axum::http::Request<Body>,
) -> Result<Json<serde_json::Value>, AppError> {
    let (_parts, body) = req.into_parts();
    let bytes = read_body_limited(body, MAX_SMALL_BODY_BYTES).await?;

    set_repo_password_with_body(&state, auth.user_id, &repo_id, &bytes).await
}

/// Shared body of the v2 set-password surfaces.
///
/// Membership is checked first, otherwise this is a password oracle for any
/// authenticated non-member; the per-(user, repo) limiter below is the brute-force
/// control, since the wire protocol fixes a low KDF iteration count.
async fn set_repo_password_with_body(
    state: &Arc<AppState>,
    user_id: i32,
    repo_id: &str,
    bytes: &[u8],
) -> Result<Json<serde_json::Value>, AppError> {
    crate::domain::permission::check_repo_read_permission(
        state.repos.member.as_ref(),
        repo_id,
        user_id,
    )
    .await?;

    if state
        .auth_limiters
        .is_repo_password_limited(user_id, repo_id)
    {
        return Err(AppError::TooManyRequests);
    }

    let password = parse_body_field(bytes, "password", "password required")?;

    if let Err(e) = PasswordService::set_password(
        &state.password_manager,
        &state.repos,
        repo_id,
        user_id,
        &password,
    )
    .await
    {
        // A wrong password is what the limiter counts; other failures (unknown
        // repo, not encrypted) are not guesses and should not consume budget.
        if matches!(e, base::error::AppError::RepoPasswdRequired) {
            state
                .auth_limiters
                .record_repo_password_failure(user_id, repo_id);
            return Err(incorrect_password());
        }
        return Err(e);
    }

    // A correct password clears the budget: clients re-submit the cached password
    // repeatedly (Android's download worker per download), so successes must not
    // accumulate.
    state
        .auth_limiters
        .clear_repo_password_failures(user_id, repo_id);

    Ok(ok_json())
}

/// `POST /api2/repos/{repo_id}/?op=checkpassword` — check an encrypted repo's
/// password (v2 API).
///
/// Verifies a caller-supplied `magic` (the password-equivalent the client derives
/// locally), so it is a password oracle and is metered with the same per-(user,
/// repo) limiter as `?op=setpassword`.
///
/// A `pwd_hash` library has no `magic` and is answered with a 400; upstream cannot
/// check one either (`strcmp` against an empty `repo->magic` always fails and
/// surfaces as a 500), and no official client calls this operation.
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

/// Extract a field from a POST body via the JSON → form → multipart probe.
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
    let peer = auth.device.as_ref().map(|device| device.peer());
    let info = service::RepoService::download_info(
        &state.repos,
        &repo_id,
        auth.user_id,
        peer.as_ref(),
        state.config().auth.sync_token_ttl_days,
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
    headers: HeaderMap,
    Path(repo_id): Path<String>,
    Query(query): Query<LinkQuery>,
) -> Result<Json<String>, AppError> {
    let parent_dir = query.p.as_deref().unwrap_or("/");

    // Use the same Host-aware base as download links: with `site_url` left at
    // its built-in loopback default, a LAN client that reached the server by
    // address must be told to upload to *that* address, not to 127.0.0.1.
    let host_header = headers.get("host").and_then(|v| v.to_str().ok());
    let base_url = state.config().server.download_url_base(host_header);

    let url = service::RepoService::get_upload_link(
        &state.repos,
        &state.token_manager,
        &base_url,
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
    headers: HeaderMap,
    Path(repo_id): Path<String>,
    Query(query): Query<LinkQuery>,
) -> Result<Json<String>, AppError> {
    let parent_dir = query.p.as_deref().unwrap_or("/");

    let host_header = headers.get("host").and_then(|v| v.to_str().ok());
    let base_url = state.config().server.download_url_base(host_header);

    let url = service::RepoService::get_update_link(
        &state.repos,
        &state.token_manager,
        &base_url,
        &repo_id,
        auth.user_id,
        &auth.email,
        parent_dir,
        query.from.as_deref(),
    )
    .await?;

    Ok(Json(url))
}

/// `GET /api2/repo-tokens/?repos=id1,id2` — batch sync tokens for the listed repos.
pub async fn repo_tokens(
    auth: AuthUser,
    State(state): State<Arc<AppState>>,
    headers: axum::http::HeaderMap,
    Query(params): Query<HashMap<String, String>>,
) -> Result<Json<HashMap<String, String>>, AppError> {
    // Minting a long-lived repo token is a state change served on GET, so a
    // cross-site navigation carrying the session cookie must not trigger it;
    // bearer callers are unaffected (cross-site requests cannot set that header).
    crate::middleware::require_csrf_for_cookie_session(&headers, &state.csrf_secret)?;

    let repos_param = params
        .get("repos")
        .ok_or_else(|| AppError::BadRequest("repos parameter required".into()))?;
    // A key bound to specific libraries may only mint tokens for those; other ids
    // are dropped rather than rejected, matching the service's visibility rules.
    let scope = auth.repo_scope();
    let repo_ids: Vec<&str> = repos_param
        .split(',')
        .map(|s| s.trim())
        .filter(|s| !s.is_empty() && scope.allows(s))
        .collect();

    let peer = auth.device.as_ref().map(|device| device.peer());
    let result = service::RepoService::repo_tokens(
        &state.repos,
        &repo_ids,
        auth.user_id,
        peer.as_ref(),
        state.config().auth.sync_token_ttl_days,
    )
    .await?;

    Ok(Json(result))
}

/// GET /api/v2.1/repos/
pub async fn list_repos_v21(
    auth: AuthUser,
    State(state): State<Arc<AppState>>,
) -> Result<Json<V21RepoListResponse>, AppError> {
    let mut response =
        service::RepoService::list_repos_v21(&state.repos, auth.user_id, &auth.email).await?;
    let scope = auth.repo_scope();
    response.repos.retain(|repo| scope.allows(&repo.repo_id));
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
/// Returns `{"exists": bool, "repo_id": <id>}` (matching seahub's `DefaultRepoView`).
pub async fn get_default_repo(
    auth: AuthUser,
    State(state): State<Arc<AppState>>,
) -> Result<Json<serde_json::Value>, AppError> {
    let default_id = service::RepoService::default_repo_id(&state.repos, auth.user_id).await?;
    let scope = auth.repo_scope();
    match default_id.filter(|id| scope.allows(id)) {
        Some(id) => Ok(Json(serde_json::json!({"exists": true, "repo_id": id}))),
        None => Ok(Json(serde_json::json!({"exists": false}))),
    }
}

/// `POST /api2/default-repo/` — find-or-create the default library, creating
/// "My Library" when the user owns no repo yet (seahub's default name).
pub async fn create_default_repo(
    auth: AuthUser,
    State(state): State<Arc<AppState>>,
) -> Result<Json<serde_json::Value>, AppError> {
    let default_id = service::RepoService::default_repo_id(&state.repos, auth.user_id).await?;
    let id = match default_id {
        Some(id) => id,
        None => {
            let peer = auth.device.as_ref().map(|device| device.peer());
            let repo_info = service::RepoService::create_repo(
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
                service::PwdHash::default(),
                None,
                &service::EncryptedLibraryPolicy::from_config(&state.config().server),
                peer.as_ref(),
                state.config().auth.sync_token_ttl_days,
            )
            .await?;
            repo_info.id
        }
    };
    state.left_panel_cache.clear_all();
    Ok(Json(serde_json::json!({"repo_id": id})))
}
