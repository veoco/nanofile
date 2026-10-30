use axum::http::{HeaderMap, HeaderName, HeaderValue, StatusCode};
use axum::{
    Json, Router,
    body::Body,
    extract::{Path, Query, State},
    http::Request,
    response::{IntoResponse, Response},
};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;

use crate::AppState;
use crate::handler::{MAX_SMALL_BODY_BYTES, ok_json, read_body_limited};
use crate::middleware::auth::AuthUser;
use crate::middleware::repo_extractor::{RepoPathRead, RepoPathWrite};
use base::error::AppError;
use base::sanitize::safe_normalize_path;
use infra::common::{DirEntry, EMPTY_SHA1};

#[derive(Deserialize)]
pub struct DirQuery {
    pub p: Option<String>,
    pub t: Option<String>,
    pub recursive: Option<String>,
    /// `reloaddir=true` returns the affected directory's listing instead of `"success"`.
    pub reloaddir: Option<String>,
}

/// Whether a query parameter spells `true` (seahub lowercases and compares).
pub(crate) fn is_true(value: Option<&str>) -> bool {
    value.is_some_and(|v| v.eq_ignore_ascii_case("true"))
}

/// Parent directory of an absolute repo path (`/a/b` → `/a`, `/a` → `/`).
pub(crate) fn parent_dir_of(path: &str) -> String {
    match path.rfind('/') {
        Some(0) | None => "/".to_string(),
        Some(i) => path[..i].to_string(),
    }
}

/// seahub's listing order (`get_dir_entrys_by_id`, `seahub/api2/views.py:2455-2460`):
/// directories first, then files, each by lowercased name. The stored order is the
/// object's descending-name identity order, which no client should be shown.
fn sort_api_entries(entries: &mut [DirEntry]) {
    entries.sort_by_cached_key(|e| (e.entry_type != "dir", e.name.to_lowercase()));
}

/// Quote the directory's `oid` header, or the empty-directory sentinel when the
/// library has no head commit yet.
///
/// seahub always has a root directory (a new library gets a "Created library" commit),
/// so its listing answers `oid=000…0` for an empty one; nanofile creates headless
/// libraries, and the official desktop client rejects a listing whose `oid` is not 40
/// characters (`GetDirentsRequest::requestSuccess`). The sentinel is the same value
/// `GET /repo/{id}/commit/HEAD` already answers for such a library.
fn oid_header(dir_id: &str) -> HeaderValue {
    HeaderValue::from_str(if dir_id.is_empty() {
        EMPTY_SHA1
    } else {
        dir_id
    })
    .unwrap_or_else(|_| HeaderValue::from_static(EMPTY_SHA1))
}

/// The headers both legacy listing shapes carry: `oid` and the caller's real
/// `dir_perm` (`"rw"`/`"r"`), which the desktop client reads to decide whether the
/// library is read-only.
fn dir_headers(dir_id: &str, permission: &'static str) -> HeaderMap {
    let mut headers = HeaderMap::new();
    headers.insert(HeaderName::from_static("oid"), oid_header(dir_id));
    headers.insert(
        HeaderName::from_static("dir_perm"),
        HeaderValue::from_static(permission),
    );
    headers
}

/// Fill what the tree walk does not know: the caller's permission on every entry and
/// the modifier display names seahub adds for files.
async fn present_api_entries(
    state: &AppState,
    mut entries: Vec<DirEntry>,
    permission: &'static str,
) -> Result<Vec<DirEntry>, AppError> {
    for entry in &mut entries {
        entry.permission = permission.to_string();
    }
    // The recursive walk already resolved the nicknames; do not pay for the batched
    // lookup twice.
    let needs_names = entries
        .iter()
        .any(|e| e.entry_type == "file" && e.modifier_name.is_none() && !e.modifier.is_empty());
    if needs_names {
        crate::service::fs::dir::fill_modifier_names(&state.repos, &mut entries).await?;
    }
    Ok(entries)
}

/// Keep the entries `t` selects (`f` files, `d` directories), seahub's meaning for the
/// parameter on both the plain and the recursive listing.
fn filter_by_type(entries: Vec<DirEntry>, request_type: Option<&str>) -> Vec<DirEntry> {
    match request_type {
        Some("f") => entries
            .into_iter()
            .filter(|e| e.entry_type == "file")
            .collect(),
        Some("d") => entries
            .into_iter()
            .filter(|e| e.entry_type == "dir")
            .collect(),
        _ => entries,
    }
}

/// Build the `reloaddir=true` response: the dirents as a **bare** JSON array plus the
/// `oid`/`dir_perm` headers seahub's `reloaddir()` produces
/// (`seahub/api2/views.py:2560-2576`). The iOS client's `SeafDir.handleData:` accepts
/// only a bare array or a `{"dirent_list": […]}` wrapper, so a `{"dir_listing": …}`
/// wrapper left its list stale and a rename reported failure on a server success.
pub(crate) async fn reload_dir_response(
    state: &AppState,
    repo_id: &str,
    path: &str,
    permission: &'static str,
) -> Result<Response, AppError> {
    let (dir_id, entries) = state.dir_service().list_dir(repo_id, path).await?;
    let mut entries = present_api_entries(state, entries, permission).await?;
    sort_api_entries(&mut entries);
    Ok((dir_headers(&dir_id, permission), Json(entries)).into_response())
}

pub fn dir_routes() -> Router<Arc<AppState>> {
    Router::new()
        .route(
            "/{repo_id}/dir/",
            axum::routing::get(list_dir)
                .post(dir_post_handler)
                .delete(delete_dir),
        )
        .route("/{repo_id}/dir/move/", axum::routing::post(move_dir))
        .route("/{repo_id}/dir/rename/", axum::routing::post(rename_dir))
        .route(
            "/{repo_id}/dir/shared_items/",
            axum::routing::get(dir_shared_items),
        )
        .route(
            "/{repo_id}/dir/sub_repo/",
            axum::routing::get(create_sub_repo),
        )
}

pub async fn list_dir(
    access: RepoPathRead,
    State(state): State<Arc<AppState>>,
    Query(query): Query<DirQuery>,
) -> Result<impl IntoResponse, AppError> {
    let repo_id = &access.repo_id;

    let path = safe_normalize_path(&query.p.unwrap_or_else(|| "/".to_string()))
        .map_err(|e| AppError::BadRequest(format!("Invalid path: {e}")))?;
    if let Some(ref r) = query.recursive
        && r != "0"
        && r != "1"
    {
        return Err(AppError::BadRequest(
            "If you want to get recursive dir entries, you should set 'recursive' argument as '1'."
                .into(),
        ));
    }
    if let Some(ref t) = query.t
        && t != "f"
        && t != "d"
    {
        return Err(AppError::BadRequest(
            "'t'(type) should be 'f' or 'd'.".into(),
        ));
    }

    let svc = state.dir_service();

    if query.recursive.as_deref() == Some("1") {
        let (dir_id, all_entries) = svc.list_dir_recursive(repo_id, &path).await?;
        let filtered = filter_by_type(all_entries, query.t.as_deref());
        // seahub's recursive listing is a depth-first walk, so its order is a traversal
        // artifact; only the permission is a contract.
        let entries = present_api_entries(&state, filtered, access.permission).await?;
        return Ok((dir_headers(&dir_id, access.permission), Json(entries)));
    }

    let (dir_id, entries) = svc.list_dir(repo_id, &path).await?;
    let entries = filter_by_type(entries, query.t.as_deref());
    let mut entries = present_api_entries(&state, entries, access.permission).await?;
    sort_api_entries(&mut entries);
    Ok((dir_headers(&dir_id, access.permission), Json(entries)))
}

pub async fn dir_post_handler(
    access: RepoPathWrite,
    State(state): State<Arc<AppState>>,
    Query(query): Query<DirQuery>,
    req: Request<Body>,
) -> Result<Response, AppError> {
    let repo_id = &access.repo_id;

    let (_parts, body) = req.into_parts();
    let bytes = read_body_limited(body, MAX_SMALL_BODY_BYTES).await?;

    let (op, p, newname): (Option<String>, Option<String>, Option<String>) =
        if let Ok(json_val) = serde_json::from_slice::<serde_json::Value>(&bytes) {
            let op = json_val
                .get("operation")
                .and_then(|v| v.as_str())
                .map(String::from);
            let p = json_val.get("p").and_then(|v| v.as_str()).map(String::from);
            let newname = json_val
                .get("newname")
                .and_then(|v| v.as_str())
                .map(String::from);
            (op, p, newname)
        } else if let Ok(form) = serde_urlencoded::from_bytes::<HashMap<String, String>>(&bytes) {
            (
                form.get("operation").cloned(),
                form.get("p").cloned(),
                form.get("newname").cloned(),
            )
        } else {
            let op = infra::common::util::extract_multipart_field(&bytes, "operation");
            let p = infra::common::util::extract_multipart_field(&bytes, "p");
            let newname = infra::common::util::extract_multipart_field(&bytes, "newname");
            (op, p, newname)
        };

    let path = p
        .or_else(|| query.p.clone())
        .ok_or_else(|| AppError::BadRequest("path required".into()))?;
    let path = safe_normalize_path(&path)
        .map_err(|e| AppError::BadRequest(format!("Invalid path: {e}")))?;

    let svc = state.dir_service();

    match op.as_deref() {
        Some("rename") => {
            let newname = newname.ok_or_else(|| AppError::BadRequest("newname required".into()))?;
            svc.rename_dir_entry(
                repo_id,
                &path,
                &newname,
                &access.user.email,
                access.user.user_id,
            )
            .await?;
            Ok(Json(serde_json::Value::String("success".to_string())).into_response())
        }
        _ => {
            svc.create_dir(repo_id, &path, &access.user.email, access.user.user_id)
                .await?;
            // iOS sends `reloaddir=true` and reads the parent listing from the
            // body; otherwise the new folder would not appear until a refresh.
            if is_true(query.reloaddir.as_deref()) {
                let parent = parent_dir_of(&path);
                return reload_dir_response(&state, repo_id, &parent, "rw").await;
            }
            // Otherwise seahub answers 201 with a `Location` for the new folder.
            let mut headers = HeaderMap::new();
            if let Ok(value) = HeaderValue::from_str(&format!(
                "/api2/repos/{repo_id}/dir/?p={}",
                percent_encoding::utf8_percent_encode(&path, percent_encoding::NON_ALPHANUMERIC)
            )) {
                headers.insert(axum::http::header::LOCATION, value);
            }
            Ok((
                StatusCode::CREATED,
                headers,
                Json(serde_json::Value::String("success".to_string())),
            )
                .into_response())
        }
    }
}

pub async fn delete_dir(
    access: RepoPathWrite,
    State(state): State<Arc<AppState>>,
    Query(query): Query<DirQuery>,
) -> Result<Response, AppError> {
    let repo_id = &access.repo_id;

    let path = safe_normalize_path(
        &query
            .p
            .ok_or_else(|| AppError::BadRequest("path is required".into()))?,
    )
    .map_err(|e| AppError::BadRequest(format!("Invalid path: {e}")))?;

    let svc = state.dir_service();
    svc.delete_dir(repo_id, &path, &access.user.email, access.user.user_id)
        .await?;

    // seahub's `DirView.delete` ends in `reloaddir_if_necessary`.
    if is_true(query.reloaddir.as_deref()) {
        return reload_dir_response(&state, repo_id, &parent_dir_of(&path), "rw").await;
    }
    Ok(ok_json().into_response())
}

#[derive(Deserialize)]
pub struct MoveDirRequest {
    pub repo_id: String,
    pub p: String,
    pub new_parent_dir: String,
}

pub async fn move_dir(
    auth: AuthUser,
    State(state): State<Arc<AppState>>,
    Path(path_repo_id): Path<String>,
    Json(req): Json<MoveDirRequest>,
) -> Result<(), AppError> {
    if path_repo_id != req.repo_id {
        return Err(AppError::BadRequest(
            "repo_id does not match the path".into(),
        ));
    }
    auth.ensure_repo_allowed(&req.repo_id, true)?;
    crate::domain::permission::check_repo_write_permission(
        state.repos.member.as_ref(),
        &req.repo_id,
        auth.user_id,
    )
    .await?;

    let path = safe_normalize_path(&req.p)
        .map_err(|e| AppError::BadRequest(format!("Invalid path: {e}")))?;

    let svc = state.dir_service();
    svc.move_dir(
        &req.repo_id,
        &path,
        &req.new_parent_dir,
        &auth.email,
        auth.user_id,
    )
    .await
}

#[derive(Deserialize)]
pub struct RenameDirRequest {
    pub repo_id: String,
    pub p: String,
    pub new_name: String,
}

pub async fn rename_dir(
    auth: AuthUser,
    State(state): State<Arc<AppState>>,
    Path(path_repo_id): Path<String>,
    Json(req): Json<RenameDirRequest>,
) -> Result<(), AppError> {
    if path_repo_id != req.repo_id {
        return Err(AppError::BadRequest(
            "repo_id does not match the path".into(),
        ));
    }
    auth.ensure_repo_allowed(&req.repo_id, true)?;
    crate::domain::permission::check_repo_write_permission(
        state.repos.member.as_ref(),
        &req.repo_id,
        auth.user_id,
    )
    .await?;

    let path = safe_normalize_path(&req.p)
        .map_err(|e| AppError::BadRequest(format!("Invalid path: {e}")))?;
    let svc = state.dir_service();
    svc.rename_dir_entry(
        &req.repo_id,
        &path,
        &req.new_name,
        &auth.email,
        auth.user_id,
    )
    .await
}

#[derive(Serialize)]
pub struct DirSharedItemsResponse {
    pub shared_items: Vec<serde_json::Value>,
}

pub async fn dir_shared_items(
    access: RepoPathRead,
    State(state): State<Arc<AppState>>,
    Query(query): Query<DirQuery>,
) -> Result<Json<DirSharedItemsResponse>, AppError> {
    let repo_id = &access.repo_id;

    let path = safe_normalize_path(&query.p.unwrap_or_else(|| "/".to_string()))
        .map_err(|e| AppError::BadRequest(format!("Invalid path: {e}")))?;
    let svc = state.dir_service();
    let items = svc.get_dir_shared_items(repo_id, &path).await?;

    Ok(Json(DirSharedItemsResponse {
        shared_items: items,
    }))
}

/// GET /api2/repos/{repo_id}/dir/sub_repo/ — create a library from a folder.
///
/// Registered on `GET` for client compatibility but it mutates state; `AuthUser` skips
/// CSRF for safe methods, so a browser cookie session would let a cross-site
/// `<img src=...>` create a library in the victim's account. Bearer-token callers
/// cannot be reached cross-site and are unaffected.
pub async fn create_sub_repo(
    access: RepoPathWrite,
    State(state): State<Arc<AppState>>,
    headers: axum::http::HeaderMap,
    Query(query): Query<DirQuery>,
) -> Result<Json<serde_json::Value>, AppError> {
    crate::middleware::require_csrf_for_cookie_session(&headers, &state.csrf_secret)?;

    let repo_id = &access.repo_id;

    let path = safe_normalize_path(
        &query
            .p
            .ok_or_else(|| AppError::BadRequest("path required".into()))?,
    )
    .map_err(|e| AppError::BadRequest(format!("Invalid path: {e}")))?;

    let svc = state.dir_service();
    let result = svc
        .create_sub_repo(repo_id, &path, &access.user.email, access.user.user_id)
        .await?;

    state.left_panel_cache.clear_all();

    Ok(Json(result))
}

#[derive(Deserialize)]
pub struct V21DirQuery {
    pub p: Option<String>,
    pub t: Option<String>,
    pub recursive: Option<String>,
    pub with_thumbnail: Option<bool>,
}

pub async fn list_dir_v21(
    access: RepoPathRead,
    State(state): State<Arc<AppState>>,
    Query(query): Query<V21DirQuery>,
) -> Result<Response, AppError> {
    let repo_id = &access.repo_id;

    let path = query.p.as_deref().unwrap_or("/");
    let normalized = safe_normalize_path(path)
        .map_err(|e| AppError::BadRequest(format!("Invalid path: {e}")))?;
    if let Some(ref r) = query.recursive
        && r != "0"
        && r != "1"
    {
        return Err(AppError::BadRequest(
            "If you want to get recursive dir entries, you should set 'recursive' argument as '1'."
                .into(),
        ));
    }
    if let Some(ref t) = query.t
        && t != "f"
        && t != "d"
    {
        return Err(AppError::BadRequest(
            "'t'(type) should be 'f' or 'd'.".into(),
        ));
    }

    let svc = state.dir_service();

    if query.recursive.as_deref() == Some("1") {
        let (dir_id, all_entries) = svc.list_dir_recursive(repo_id, &normalized).await?;
        let dirent_list = filter_by_type(all_entries, query.t.as_deref());
        // seahub's recursive listing is a depth-first walk, so its order is a traversal
        // artifact; iOS and Android sort locally, and the permission is the contract.
        let dirent_list = present_api_entries(&state, dirent_list, access.permission).await?;

        let body = serde_json::json!({
            "user_perm": access.permission,
            "dir_id": if dir_id.is_empty() { EMPTY_SHA1 } else { &dir_id },
            "dirent_list": dirent_list,
        });
        return Ok((dir_headers(&dir_id, access.permission), Json(body)).into_response());
    }

    let (dir_id, entries) = svc.list_dir(repo_id, &normalized).await?;

    let json_body = svc
        .build_list_dir_v21_json(
            repo_id,
            &normalized,
            access.user.user_id,
            access.permission,
            query.with_thumbnail.unwrap_or(false),
            entries,
            if dir_id.is_empty() {
                EMPTY_SHA1.to_string()
            } else {
                dir_id.clone()
            },
        )
        .await?;

    Ok((dir_headers(&dir_id, access.permission), Json(json_body)).into_response())
}

#[derive(Deserialize)]
pub struct CreateDirBody {
    p: Option<String>,
    #[serde(rename = "operation")]
    _operation: Option<String>,
}

pub async fn create_dir_v21(
    access: RepoPathWrite,
    State(state): State<Arc<AppState>>,
    Query(query): Query<V21DirQuery>,
    Json(body): Json<CreateDirBody>,
) -> Result<Json<serde_json::Value>, AppError> {
    let repo_id = &access.repo_id;

    let path = body
        .p
        .or(query.p)
        .ok_or_else(|| AppError::BadRequest("path (p) required".into()))?;
    let path = safe_normalize_path(&path)
        .map_err(|e| AppError::BadRequest(format!("Invalid path: {e}")))?;

    let svc = state.dir_service();
    svc.create_dir(repo_id, &path, &access.user.email, access.user.user_id)
        .await?;

    Ok(ok_json())
}

#[derive(Deserialize)]
pub struct DirDetailQuery {
    pub path: Option<String>,
}

pub async fn dir_detail_v21(
    access: RepoPathRead,
    State(state): State<Arc<AppState>>,
    Query(query): Query<DirDetailQuery>,
) -> Result<Json<serde_json::Value>, AppError> {
    let repo_id = &access.repo_id;

    let path = query
        .path
        .ok_or_else(|| AppError::BadRequest("path required".into()))?;
    if path == "/" || path.is_empty() {
        return Err(AppError::BadRequest("path invalid.".into()));
    }
    let normalized = safe_normalize_path(&path)
        .map_err(|e| AppError::BadRequest(format!("Invalid path: {e}")))?;

    let svc = state.dir_service();
    let result = svc
        .dir_detail(repo_id, &normalized, access.permission)
        .await?;

    Ok(Json(result))
}
