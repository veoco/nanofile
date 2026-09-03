use askama::Template;
use axum::{
    body::Body,
    extract::{ConnectInfo, Path, Query, State},
    http::{HeaderMap, HeaderName, HeaderValue, StatusCode, header},
    response::{Html, IntoResponse, Response},
};
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;

use crate::AppState;
use crate::fs::core::download::Downloader;
use crate::fs::core::tree::{read_fs_dir_data, resolve_fs_id};
use crate::fs::zip::{ZipLimits, collect_dir_entries, stream_zip};
use crate::i18n::I18n;
use crate::ui::format_size;
use base::common::FsFileData;
use base::error::AppError;
use infra::common::S_IFDIR;

// ── Templates ─────────────────────────────────────────────────────────────

#[derive(Template)]
#[template(path = "web/share_view.html")]
struct ShareViewTemplate {
    pub t: &'static I18n,
    pub file_name: String,
    pub file_ext: String,
    pub file_size: String,
    pub has_password: bool,
    pub created_at_ts: i64,
    pub expires_at_ts: Option<i64>,
    pub download_url: String,
    pub description: Option<String>,
}

#[allow(dead_code)]
#[derive(Template)]
#[template(path = "web/share_access_validation.html")]
struct ShareAccessValidationTemplate {
    pub t: &'static I18n,
    pub token: String,
    pub error: Option<String>,
    pub form_action: String,
}

// ── Handler helpers ───────────────────────────────────────────────────────

/// Rate-limit anonymous share-link downloads per client IP (these endpoints
/// are reachable with just the share token and no login).
fn check_share_download_rate(
    state: &Arc<AppState>,
    addr: &SocketAddr,
    headers: &HeaderMap,
) -> Result<(), AppError> {
    let client_ip =
        crate::middleware::effective_client_ip(addr, headers, &state.config.server.trusted_proxies);
    let key = format!("share_download:{client_ip}");
    if state.auth_limiters.share_download.is_limited(&key) {
        return Err(AppError::TooManyRequests);
    }
    state.auth_limiters.share_download.record_attempt(&key);
    Ok(())
}

/// Resolve file metadata from the repo.
async fn resolve_file_meta(
    repos: &crate::repository::Repositories,
    repo_id: &str,
    path: &str,
) -> Result<(FsFileData, Vec<String>), AppError> {
    Downloader::resolve_blocks(repos, repo_id, path)
        .await
        .map_err(|_| AppError::NotFound("File not found".into()))
}

// ── Main GET handler ──────────────────────────────────────────────────────

/// GET /f/{token}/ — show HTML preview or download file.
pub async fn shared_file_view(
    State(state): State<Arc<AppState>>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    Path(token): Path<String>,
    headers: HeaderMap,
    Query(params): Query<HashMap<String, String>>,
) -> Result<Response, AppError> {
    let link = crate::service::sharing::share::resolve_share_link(&state.repos, &token).await?;

    // Password check
    let provided_pwd = headers
        .get("X-Seafile-Sharelink-Password")
        .and_then(|v| v.to_str().ok())
        .or_else(|| params.get("password").map(|s| s.as_str()));
    let pw_ok = crate::service::sharing::share::check_share_link_password(
        &link,
        provided_pwd,
        state.config.auth.password_hash_iterations,
    )
    .await?;

    // If password is required but not provided, show password form
    if link.password.is_some() && !pw_ok {
        // Check if this is a POST-back with wrong password
        let error = if params.contains_key("password") {
            Some("Incorrect password".to_string())
        } else {
            None
        };
        let tpl = ShareAccessValidationTemplate {
            t: I18n::from_headers(&headers, &state.config.ui.default_language),
            token: token.clone(),
            error,
            form_action: format!("/f/{}/", token),
        };
        let html = tpl
            .render()
            .map_err(|e| AppError::Internal(e.to_string()))?;
        return Ok(Html(html).into_response());
    }

    // Handle ?dl=1 — download the file directly
    if params.get("dl").map(|s| s.as_str()) == Some("1") {
        check_share_download_rate(&state, &addr, &headers)?;
        let (file_data, block_ids) =
            resolve_file_meta(&state.repos, &link.repo_id, &link.path).await?;

        let filename = link
            .path
            .rsplit_once('/')
            .map(|(_, n)| n)
            .unwrap_or(&link.path)
            .to_string();

        crate::service::sharing::share::increment_view_cnt(state.repos.share_link.clone(), link.id);

        let disposition = format!("attachment; filename=\"{}\"", filename);
        let range_header = headers.get(header::RANGE).and_then(|v| v.to_str().ok());
        return Ok(crate::fs::core::download::file_download_response(
            crate::fs::core::download::FileDownloadParams {
                block_ids,
                block_store: state.block_store.clone(),
                enc_key: None,
                total_size: file_data.size.max(0) as u64,
                content_type: "application/octet-stream",
                content_disposition: Some(disposition),
                range_header: range_header.map(|s| s.to_string()),
                etag: None,
            },
        ));
    }

    // Show HTML preview page
    let (_file_data, _block_ids) =
        resolve_file_meta(&state.repos, &link.repo_id, &link.path).await?;

    crate::service::sharing::share::increment_view_cnt(state.repos.share_link.clone(), link.id);

    let file_name = link
        .path
        .rsplit_once('/')
        .map(|(_, n)| n)
        .unwrap_or(&link.path)
        .to_string();
    let file_ext = file_name
        .rsplit_once('.')
        .map(|(_, e)| e.to_string())
        .unwrap_or_else(|| "?".to_string());
    let file_size = _file_data.size;

    let mut download_url = format!("/f/{}/?dl=1", link.token);
    // Pass password through to download URL if provided
    if let Some(pwd) = params.get("password") {
        download_url.push_str(&format!("&password={}", pwd));
    }

    let tpl = ShareViewTemplate {
        t: I18n::from_headers(&headers, &state.config.ui.default_language),
        file_name: file_name.clone(),
        file_ext,
        file_size: format_size(file_size),
        has_password: link.password.is_some(),
        created_at_ts: link.created_at,
        expires_at_ts: link.expires_at,
        download_url,
        description: link.description.clone(),
    };

    let html = tpl
        .render()
        .map_err(|e| AppError::Internal(e.to_string()))?;
    Ok(Html(html).into_response())
}

// ── POST handler for password submission ──────────────────────────────────

/// POST /f/{token}/ — validate password, redirect with password in URL.
pub async fn shared_file_view_post(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    Path(token): Path<String>,
    axum::Form(form): axum::Form<HashMap<String, String>>,
) -> Result<Response, AppError> {
    let link = crate::service::sharing::share::resolve_share_link(&state.repos, &token).await?;

    let password = form
        .get("password")
        .ok_or_else(|| AppError::BadRequest("password required".into()))?;

    // Rate limit password attempts per client IP.
    let client_ip = crate::middleware::effective_client_ip(
        &addr,
        &headers,
        &state.config.server.trusted_proxies,
    );
    let rl_key = format!("link_password:{client_ip}");
    if state.auth_limiters.link_password.is_limited(&rl_key) {
        return Err(AppError::TooManyRequests);
    }
    state.auth_limiters.link_password.record_attempt(&rl_key);

    let valid = crate::service::auth::password::verify_password_async(
        password.clone(),
        link.password.clone().unwrap_or_default(),
        state.config.auth.password_hash_iterations,
    )
    .await;

    if !valid {
        // Show password form again with error
        let tpl = ShareAccessValidationTemplate {
            t: I18n::from_headers(&headers, &state.config.ui.default_language),
            token: token.clone(),
            error: Some("Incorrect password".to_string()),
            form_action: format!("/f/{}/", token),
        };
        let html = tpl
            .render()
            .map_err(|e| AppError::Internal(e.to_string()))?;
        return Ok(Html(html).into_response());
    }

    // Redirect to GET with password in query param
    let redirect = format!("/f/{}/?password={}", token, urlencoding(password));
    Ok((StatusCode::FOUND, [("Location", redirect.as_str())]).into_response())
}

// ── Directory share ──────────────────────────────────────────────────

#[derive(Template)]
#[template(path = "web/shared_dir_view.html")]
struct SharedDirViewTemplate {
    pub t: &'static I18n,
    pub token: String,
    pub dir_name: String,
    pub dir_path: String,
    pub parent_path: Option<String>,
    pub entries: Vec<DirEntryInfo>,
    pub item_count: usize,
    pub has_password: bool,
    pub created_at_ts: i64,
    pub expires_at_ts: Option<i64>,
    pub download_url: String,
    pub password_query: String,
    pub description: Option<String>,
    pub page: u32,
    pub total_pages: usize,
    pub has_more: bool,
}

#[derive(Clone)]
struct DirEntryInfo {
    pub name: String,
    pub ext: String,
    pub is_dir: bool,
    pub size: String,
    pub full_path: String,
}

/// GET /d/{token}/ — show directory file listing, or ?dl=1 to download ZIP.
pub async fn shared_dir_view(
    State(state): State<Arc<AppState>>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    Path(token): Path<String>,
    headers: HeaderMap,
    Query(params): Query<HashMap<String, String>>,
) -> Result<Response, AppError> {
    let link = crate::service::sharing::share::resolve_share_link(&state.repos, &token).await?;

    // Only handle directory shares
    if link.s_type != "d" {
        return Err(AppError::NotFound("Not a directory share link".into()));
    }

    // Password check (same as file share)
    let provided_pwd = headers
        .get("X-Seafile-Sharelink-Password")
        .and_then(|v| v.to_str().ok())
        .or_else(|| params.get("password").map(|s| s.as_str()));
    let pw_ok = crate::service::sharing::share::check_share_link_password(
        &link,
        provided_pwd,
        state.config.auth.password_hash_iterations,
    )
    .await?;

    if link.password.is_some() && !pw_ok {
        let error = if params.contains_key("password") {
            Some("Incorrect password".to_string())
        } else {
            None
        };
        let tpl = ShareAccessValidationTemplate {
            t: I18n::from_headers(&headers, &state.config.ui.default_language),
            token: token.clone(),
            error,
            form_action: format!("/d/{}/", token),
        };
        let html = tpl
            .render()
            .map_err(|e| AppError::Internal(e.to_string()))?;
        return Ok(Html(html).into_response());
    }

    // Get repo head commit
    let repo_model = state
        .repos
        .repo
        .find_by_id(&link.repo_id)
        .await?
        .ok_or_else(|| AppError::NotFound("Repo not found".into()))?;
    let head_commit_id = repo_model
        .head_commit_id
        .ok_or_else(|| AppError::BadRequest("Repo has no commits".into()))?;
    let head_commit = state
        .repos
        .commit
        .find_by_id(&head_commit_id)
        .await?
        .ok_or_else(|| AppError::Internal("Head commit not found".into()))?;

    // Handle ?dl=1 — download entire directory as ZIP
    if params.get("dl").map(|s| s.as_str()) == Some("1") {
        check_share_download_rate(&state, &addr, &headers)?;
        let dir_name = link
            .path
            .rsplit_once('/')
            .map(|(_, n)| n)
            .unwrap_or(&link.path)
            .to_string();
        let dir_name = if dir_name.is_empty() {
            "download".to_string()
        } else {
            dir_name
        };
        let files = collect_dir_entries(
            &state.repos,
            &link.repo_id,
            &head_commit.root_id,
            &link.path,
            &dir_name,
            ZipLimits {
                max_entries: state.config.storage.max_zip_entries,
                max_bytes: state.config.storage.max_zip_bytes,
            },
        )
        .await?;

        crate::service::sharing::share::increment_view_cnt(state.repos.share_link.clone(), link.id);
        let stream = stream_zip(state.block_store.clone(), files, None);

        let mut headers = HeaderMap::new();
        headers.insert(
            header::CONTENT_TYPE,
            HeaderValue::from_static("application/zip"),
        );
        headers.insert(
            HeaderName::from_static("content-disposition"),
            HeaderValue::from_str(&format!("attachment; filename=\"{}.zip\"", dir_name))
                .unwrap_or_else(|_| HeaderValue::from_static("attachment")),
        );
        return Ok((StatusCode::OK, headers, Body::from_stream(stream)).into_response());
    }

    crate::service::sharing::share::increment_view_cnt(state.repos.share_link.clone(), link.id);

    // Resolve the current directory path using safe path joining
    // to prevent path traversal attacks (e.g., ?p=../other-dir)
    let sub_path = params.get("p").map(|s| s.as_str()).unwrap_or("/");
    let current_path = base::sanitize::safe_join_path(&link.path, sub_path)
        .map_err(|e| AppError::BadRequest(format!("Invalid path: {e}")))?;

    // `safe_join_path` only clamps traversal to the repo root, so `?p=../../`
    // could climb above the shared directory. Keep the resolved path inside the
    // shared subtree (unless the whole repo is shared).
    let share_root =
        base::sanitize::safe_normalize_path(&link.path).unwrap_or_else(|_| "/".to_string());
    if share_root != "/"
        && current_path != share_root
        && !current_path.starts_with(&format!("{}/", share_root.trim_end_matches('/')))
    {
        return Err(AppError::BadRequest("Invalid path".into()));
    }

    let dir_id = resolve_fs_id(
        &state.repos,
        &link.repo_id,
        &head_commit.root_id,
        &current_path,
    )
    .await
    .map_err(|_| AppError::NotFound("Directory not found".into()))?;

    let dir_data = read_fs_dir_data(&state.repos, &link.repo_id, &dir_id)
        .await
        .map_err(|_| AppError::NotFound("Directory not found".into()))?;

    // Build entry list
    let mut entries: Vec<DirEntryInfo> = Vec::new();
    for dirent in &dir_data.dirents {
        let is_dir = dirent.mode & S_IFDIR != 0;
        let size = if is_dir { 0 } else { dirent.size };
        let full_path = if sub_path == "/" {
            format!("/{}", dirent.name)
        } else {
            format!("{}/{}", sub_path.trim_end_matches('/'), dirent.name)
        };

        let ext = if is_dir {
            String::new()
        } else {
            dirent
                .name
                .rsplit_once('.')
                .map(|(_, e)| e.to_string())
                .unwrap_or_default()
        };
        entries.push(DirEntryInfo {
            name: dirent.name.clone(),
            ext,
            is_dir,
            size: format_size(size),
            full_path,
        });
    }

    // Sort: directories first, then files, alphabetically
    entries.sort_by(|a, b| {
        if a.is_dir != b.is_dir {
            b.is_dir.cmp(&a.is_dir)
        } else {
            a.name.cmp(&b.name)
        }
    });

    // Paginate on the sorted list so a huge folder doesn't render one
    // multi-MB page. `total` still drives the summary count; only the current
    // page slice is sent to the template.
    let total = entries.len();
    const PER_PAGE: usize = 200;
    let page = params
        .get("page")
        .and_then(|s| s.parse::<u32>().ok())
        .unwrap_or(1)
        .max(1);
    let start = ((page as usize - 1) * PER_PAGE).min(total);
    let end = (start + PER_PAGE).min(total);
    let has_more = end < total;
    let total_pages = total.div_ceil(PER_PAGE);
    let entries = if start < total {
        entries[start..end].to_vec()
    } else {
        Vec::new()
    };

    let dir_name = current_path
        .rsplit_once('/')
        .map(|(_, n)| n.to_string())
        .unwrap_or_else(|| current_path.clone());
    let dir_name = if dir_name.is_empty() {
        "/".to_string()
    } else {
        dir_name
    };

    let parent_path = if sub_path != "/" {
        let parent = sub_path
            .trim_end_matches('/')
            .rsplit_once('/')
            .map(|(p, _)| {
                if p.is_empty() {
                    "/".to_string()
                } else {
                    p.to_string()
                }
            })
            .unwrap_or_else(|| "/".to_string());
        Some(parent)
    } else {
        None
    };

    let item_count = total;

    let pw_query = if let Some(pwd) = params.get("password") {
        format!("&password={}", pwd)
    } else {
        String::new()
    };

    let download_url = format!(
        "/d/{}/?dl=1{}",
        link.token,
        if pw_query.is_empty() {
            String::new()
        } else {
            format!("&{}", &pw_query[1..])
        }
    );
    let tpl = SharedDirViewTemplate {
        t: I18n::from_headers(&headers, &state.config.ui.default_language),
        token: link.token.clone(),
        dir_name,
        dir_path: sub_path.to_string(),
        parent_path,
        entries,
        item_count,
        has_password: link.password.is_some(),
        created_at_ts: link.created_at,
        expires_at_ts: link.expires_at,
        download_url,
        password_query: pw_query,
        description: link.description.clone(),
        page,
        total_pages,
        has_more,
    };

    let html = tpl
        .render()
        .map_err(|e| AppError::Internal(e.to_string()))?;
    Ok(Html(html).into_response())
}

/// GET /d/{token}/files/{*path} — download a file from a shared directory.
pub async fn shared_dir_file_view(
    State(state): State<Arc<AppState>>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    Path((token, file_path)): Path<(String, String)>,
    headers: HeaderMap,
    Query(params): Query<HashMap<String, String>>,
) -> Result<Response, AppError> {
    let link = crate::service::sharing::share::resolve_share_link(&state.repos, &token).await?;

    if link.s_type != "d" {
        return Err(AppError::NotFound("Not a directory share link".into()));
    }

    // Password check
    let provided_pwd = headers
        .get("X-Seafile-Sharelink-Password")
        .and_then(|v| v.to_str().ok())
        .or_else(|| params.get("password").map(|s| s.as_str()));
    let pw_ok = crate::service::sharing::share::check_share_link_password(
        &link,
        provided_pwd,
        state.config.auth.password_hash_iterations,
    )
    .await?;
    if link.password.is_some() && !pw_ok {
        return if params.contains_key("password") {
            Err(AppError::Forbidden)
        } else {
            Err(AppError::BadRequest("password required".into()))
        };
    }

    check_share_download_rate(&state, &addr, &headers)?;

    // Combine share path with requested file path
    let full_path = if file_path.starts_with('/') {
        format!("{}{}", link.path.trim_end_matches('/'), file_path)
    } else {
        format!("{}/{}", link.path.trim_end_matches('/'), file_path)
    };

    let (file_data, block_ids) = resolve_file_meta(&state.repos, &link.repo_id, &full_path).await?;

    crate::service::sharing::share::increment_view_cnt(state.repos.share_link.clone(), link.id);

    let filename = full_path
        .rsplit_once('/')
        .map(|(_, n)| n.to_string())
        .unwrap_or_else(|| full_path.clone());
    let disposition = format!("attachment; filename=\"{}\"", filename);
    let range_header = headers.get(header::RANGE).and_then(|v| v.to_str().ok());
    Ok(crate::fs::core::download::file_download_response(
        crate::fs::core::download::FileDownloadParams {
            block_ids,
            block_store: state.block_store.clone(),
            enc_key: None,
            total_size: file_data.size.max(0) as u64,
            content_type: "application/octet-stream",
            content_disposition: Some(disposition),
            range_header: range_header.map(|s| s.to_string()),
            etag: None,
        },
    ))
}

// ── POST handler for directory share password ─────────────────────────

/// POST /d/{token}/ — validate password, redirect with password in URL.
pub async fn shared_dir_view_post(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    Path(token): Path<String>,
    axum::Form(form): axum::Form<HashMap<String, String>>,
) -> Result<Response, AppError> {
    let link = crate::service::sharing::share::resolve_share_link(&state.repos, &token).await?;

    let password = form
        .get("password")
        .ok_or_else(|| AppError::BadRequest("password required".into()))?;

    // Rate limit password attempts per client IP.
    let client_ip = crate::middleware::effective_client_ip(
        &addr,
        &headers,
        &state.config.server.trusted_proxies,
    );
    let rl_key = format!("link_password:{client_ip}");
    if state.auth_limiters.link_password.is_limited(&rl_key) {
        return Err(AppError::TooManyRequests);
    }
    state.auth_limiters.link_password.record_attempt(&rl_key);

    let valid = crate::service::auth::password::verify_password_async(
        password.clone(),
        link.password.clone().unwrap_or_default(),
        state.config.auth.password_hash_iterations,
    )
    .await;

    if !valid {
        let tpl = ShareAccessValidationTemplate {
            t: I18n::from_headers(&headers, &state.config.ui.default_language),
            token: token.clone(),
            error: Some("Incorrect password".to_string()),
            form_action: format!("/d/{}/", token),
        };
        let html = tpl
            .render()
            .map_err(|e| AppError::Internal(e.to_string()))?;
        return Ok(Html(html).into_response());
    }

    let redirect = format!("/d/{}/?password={}", token, urlencoding(password));
    Ok((StatusCode::FOUND, [("Location", redirect.as_str())]).into_response())
}

/// Simple URL encoding for password (only encode the special chars).
fn urlencoding(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char);
            }
            b' ' => out.push_str("%20"),
            _ => {
                out.push_str(&format!("%{:02X}", b));
            }
        }
    }
    out
}
