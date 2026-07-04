use askama::Template;
use axum::{
    body::Body,
    extract::{Path, Query, State},
    http::{HeaderMap, HeaderName, HeaderValue, StatusCode, header},
    response::{Html, IntoResponse, Response},
};
use chrono::TimeZone;
use futures::{Stream, StreamExt};
use sea_orm::{ColumnTrait, EntityTrait, QueryFilter, Set};
use std::collections::HashMap;
use std::sync::Arc;

use crate::AppState;
use crate::common::S_IFDIR;
use crate::entity::share_link;
use crate::error::AppError;
use crate::repo::download::Downloader;
use crate::repo::fs_tree::{read_fs_dir_data, resolve_fs_id};
use crate::serialization::fs_json::FsFileData;

// ── Stream blocks helper (copied from download.rs) ────────────────────────

fn stream_blocks(
    block_ids: Vec<String>,
    block_store: crate::storage::DynBlockStorage,
    enc_key: Option<(Vec<u8>, Vec<u8>)>,
) -> impl Stream<Item = Result<bytes::Bytes, std::io::Error>> + 'static {
    futures::stream::iter(block_ids.into_iter().map(move |block_id| {
        let store = block_store.clone();
        let key = enc_key.clone();
        async move {
            let data = store
                .read_block(&block_id)
                .await
                .map_err(|e| std::io::Error::other(e.to_string()))?;
            let data = match &key {
                Some((k, iv)) => crate::crypto::random_key::decrypt_block(&data, k, iv)
                    .map_err(|e| std::io::Error::other(e.to_string()))?,
                None => data,
            };
            Ok(bytes::Bytes::from(data))
        }
    }))
    .buffered(4)
}

// ── Templates ─────────────────────────────────────────────────────────────

#[derive(Template)]
#[template(path = "web/share_view.html")]
struct ShareViewTemplate {
    pub file_name: String,
    pub file_ext: String,
    pub file_size: String,
    pub has_password: bool,
    pub expires_at_display: String,
    pub view_cnt: i64,
    pub created_at_display: String,
    pub download_url: String,
}

#[derive(Template)]
#[template(path = "web/share_access_validation.html")]
struct ShareAccessValidationTemplate {
    pub token: String,
    pub error: Option<String>,
}

// ── Handler helpers ───────────────────────────────────────────────────────

fn format_size(size: i64) -> String {
    const UNITS: &[&str] = &["B", "KB", "MB", "GB", "TB"];
    let mut s = size as f64;
    let mut unit = 0;
    while s >= 1024.0 && unit < UNITS.len() - 1 {
        s /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{} {}", size, UNITS[unit])
    } else {
        format!("{:.1} {}", s, UNITS[unit])
    }
}

fn format_timestamp(ts: i64) -> String {
    let dt = chrono::Utc.timestamp_opt(ts, 0).unwrap();
    dt.format("%Y-%m-%d %H:%M").to_string()
}

/// Look up share link, check expiry, return the link model or error.
async fn resolve_share_link(
    db: &sea_orm::DatabaseConnection,
    token: &str,
) -> Result<share_link::Model, AppError> {
    let link = share_link::Entity::find()
        .filter(share_link::Column::Token.eq(token))
        .one(db)
        .await?
        .ok_or_else(|| AppError::NotFound("Link not found".into()))?;

    // Check expiry
    if let Some(expires_at) = link.expires_at
        && chrono::Utc::now().timestamp() > expires_at
    {
        return Err(AppError::NotFound("Link has expired".into()));
    }

    Ok(link)
}

/// Check whether the password in the request matches the stored hash.
fn check_password(
    link: &share_link::Model,
    headers: &HeaderMap,
    params: &HashMap<String, String>,
    password_hash_iterations: u32,
) -> Result<bool, AppError> {
    let stored_hash = match link.password {
        Some(ref h) => h,
        None => return Ok(true), // no password required
    };

    let provided = headers
        .get("X-Seafile-Sharelink-Password")
        .and_then(|v| v.to_str().ok().map(|s| s.to_string()))
        .or_else(|| params.get("password").cloned());

    match provided {
        Some(pwd) => Ok(crate::auth::password::verify_password(
            &pwd,
            stored_hash,
            password_hash_iterations,
        )),
        None => Ok(false),
    }
}

/// Increment view count asynchronously.
fn increment_view_cnt(db: Arc<sea_orm::DatabaseConnection>, link_id: i32) {
    tokio::spawn(async move {
        if let Ok(Some(link)) = share_link::Entity::find_by_id(link_id).one(&*db).await {
            let mut active: share_link::ActiveModel = link.into();
            let current = match &active.view_cnt {
                Set(v) => *v,
                _ => 0,
            };
            active.view_cnt = Set(current + 1);
            let _ = share_link::Entity::update(active).exec(&*db).await;
        }
    });
}

/// Resolve file metadata from the repo.
async fn resolve_file_meta(
    db: &sea_orm::DatabaseConnection,
    repo_id: &str,
    path: &str,
) -> Result<(FsFileData, Vec<String>), AppError> {
    Downloader::resolve_blocks(db, repo_id, path)
        .await
        .map_err(|_| AppError::NotFound("File not found".into()))
}

// ── Main GET handler ──────────────────────────────────────────────────────

/// GET /f/{token}/ — show HTML preview or download file.
pub async fn shared_file_view(
    State(state): State<Arc<AppState>>,
    Path(token): Path<String>,
    headers: HeaderMap,
    Query(params): Query<HashMap<String, String>>,
) -> Result<Response, AppError> {
    let link = resolve_share_link(state.db.as_ref(), &token).await?;

    // Password check
    let pw_ok = check_password(
        &link,
        &headers,
        &params,
        state.config.auth.password_hash_iterations,
    )?;

    // If password is required but not provided, show password form
    if link.password.is_some() && !pw_ok {
        // Check if this is a POST-back with wrong password
        let error = if params.contains_key("password") {
            Some("Incorrect password".to_string())
        } else {
            None
        };
        let tpl = ShareAccessValidationTemplate {
            token: token.clone(),
            error,
        };
        let html = tpl
            .render()
            .map_err(|e| AppError::Internal(e.to_string()))?;
        return Ok(Html(html).into_response());
    }

    // Handle ?dl=1 — download the file directly
    if params.get("dl").map(|s| s.as_str()) == Some("1") {
        let (_file_data, block_ids) =
            resolve_file_meta(state.db.as_ref(), &link.repo_id, &link.path).await?;

        let filename = link
            .path
            .rsplit_once('/')
            .map(|(_, n)| n)
            .unwrap_or(&link.path);
        let stream = stream_blocks(block_ids, state.block_store.clone(), None);

        increment_view_cnt(state.db.clone(), link.id);

        let mut headers = HeaderMap::new();
        headers.insert(
            header::CONTENT_TYPE,
            HeaderValue::from_static("application/octet-stream"),
        );
        headers.insert(
            HeaderName::from_static("content-disposition"),
            HeaderValue::from_str(&format!("attachment; filename=\"{}\"", filename))
                .unwrap_or_else(|_| HeaderValue::from_static("attachment")),
        );

        return Ok((StatusCode::OK, headers, Body::from_stream(stream)).into_response());
    }

    // Show HTML preview page
    let (_file_data, _block_ids) =
        resolve_file_meta(state.db.as_ref(), &link.repo_id, &link.path).await?;

    increment_view_cnt(state.db.clone(), link.id);

    // Re-fetch to get updated view_cnt
    let updated_link = share_link::Entity::find()
        .filter(share_link::Column::Token.eq(&token))
        .one(state.db.as_ref())
        .await?
        .ok_or_else(|| AppError::NotFound("Link not found".into()))?;

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
    let expires_display = match link.expires_at {
        Some(ts) => format_timestamp(ts),
        None => "Never".to_string(),
    };
    let created_display = format_timestamp(link.created_at);

    let mut download_url = format!("/f/{}/?dl=1", link.token);
    // Pass password through to download URL if provided
    if let Some(pwd) = params.get("password") {
        download_url.push_str(&format!("&password={}", pwd));
    }

    let tpl = ShareViewTemplate {
        file_name: file_name.clone(),
        file_ext,
        file_size: format_size(file_size),
        has_password: link.password.is_some(),
        expires_at_display: expires_display,
        view_cnt: updated_link.view_cnt,
        created_at_display: created_display,
        download_url,
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
    Path(token): Path<String>,
    axum::Form(form): axum::Form<HashMap<String, String>>,
) -> Result<Response, AppError> {
    let link = resolve_share_link(state.db.as_ref(), &token).await?;

    let password = form
        .get("password")
        .ok_or_else(|| AppError::BadRequest("password required".into()))?;

    let valid = crate::auth::password::verify_password(
        password,
        &link.password.unwrap_or_default(),
        state.config.auth.password_hash_iterations,
    );

    if !valid {
        // Show password form again with error
        let tpl = ShareAccessValidationTemplate {
            token: token.clone(),
            error: Some("Incorrect password".to_string()),
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
    pub token: String,
    pub dir_name: String,
    pub dir_path: String,
    pub parent_path: Option<String>,
    pub entries: Vec<DirEntryInfo>,
    pub has_password: bool,
    pub expires_at_display: String,
    pub password_query: String,
}

struct DirEntryInfo {
    pub name: String,
    pub ext: String,
    pub is_dir: bool,
    pub size: String,
    pub mtime: String,
    pub full_path: String,
}

/// GET /d/{token}/ — show directory file listing.
pub async fn shared_dir_view(
    State(state): State<Arc<AppState>>,
    Path(token): Path<String>,
    headers: HeaderMap,
    Query(params): Query<HashMap<String, String>>,
) -> Result<Response, AppError> {
    let link = resolve_share_link(state.db.as_ref(), &token).await?;

    // Only handle directory shares
    if link.s_type != "d" {
        return Err(AppError::NotFound("Not a directory share link".into()));
    }

    // Password check (same as file share)
    let pw_ok = check_password(
        &link,
        &headers,
        &params,
        state.config.auth.password_hash_iterations,
    )?;

    if link.password.is_some() && !pw_ok {
        let error = if params.contains_key("password") {
            Some("Incorrect password".to_string())
        } else {
            None
        };
        let tpl = ShareAccessValidationTemplate {
            token: token.clone(),
            error,
        };
        let html = tpl
            .render()
            .map_err(|e| AppError::Internal(e.to_string()))?;
        return Ok(Html(html).into_response());
    }

    increment_view_cnt(state.db.clone(), link.id);

    // Resolve the current directory path
    let sub_path = params.get("p").map(|s| s.as_str()).unwrap_or("/");
    let current_path = if sub_path == "/" {
        link.path.clone()
    } else {
        let trimmed = sub_path.trim_start_matches('/');
        format!("{}/{}", link.path.trim_end_matches('/'), trimmed)
    };

    // Get repo head commit and resolve directory
    let repo_model = crate::entity::repo::Entity::find_by_id(&link.repo_id)
        .one(state.db.as_ref())
        .await?
        .ok_or_else(|| AppError::NotFound("Repo not found".into()))?;
    let head_commit_id = repo_model
        .head_commit_id
        .ok_or_else(|| AppError::BadRequest("Repo has no commits".into()))?;
    let head_commit = crate::entity::commit::Entity::find()
        .filter(crate::entity::commit::Column::CommitId.eq(&head_commit_id))
        .one(state.db.as_ref())
        .await?
        .ok_or_else(|| AppError::Internal("Head commit not found".into()))?;

    let dir_id = resolve_fs_id(
        state.db.as_ref(),
        &link.repo_id,
        &head_commit.root_id,
        &current_path,
    )
    .await
    .map_err(|_| AppError::NotFound("Directory not found".into()))?;

    let dir_data = read_fs_dir_data(state.db.as_ref(), &link.repo_id, &dir_id)
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
            mtime: format_timestamp(dirent.mtime),
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

    let expires_display = match link.expires_at {
        Some(ts) => format_timestamp(ts),
        None => "Never".to_string(),
    };

    let pw_query = if let Some(pwd) = params.get("password") {
        format!("&password={}", pwd)
    } else {
        String::new()
    };

    let tpl = SharedDirViewTemplate {
        token: link.token.clone(),
        dir_name,
        dir_path: sub_path.to_string(),
        parent_path,
        entries,
        has_password: link.password.is_some(),
        expires_at_display: expires_display,
        password_query: pw_query,
    };

    let html = tpl
        .render()
        .map_err(|e| AppError::Internal(e.to_string()))?;
    Ok(Html(html).into_response())
}

/// GET /d/{token}/files/{*path} — download a file from a shared directory.
pub async fn shared_dir_file_view(
    State(state): State<Arc<AppState>>,
    Path((token, file_path)): Path<(String, String)>,
    headers: HeaderMap,
    Query(params): Query<HashMap<String, String>>,
) -> Result<Response, AppError> {
    let link = resolve_share_link(state.db.as_ref(), &token).await?;

    if link.s_type != "d" {
        return Err(AppError::NotFound("Not a directory share link".into()));
    }

    // Password check
    let pw_ok = check_password(
        &link,
        &headers,
        &params,
        state.config.auth.password_hash_iterations,
    )?;
    if link.password.is_some() && !pw_ok {
        return if params.contains_key("password") {
            Err(AppError::Forbidden)
        } else {
            Err(AppError::BadRequest("password required".into()))
        };
    }

    // Combine share path with requested file path
    let full_path = if file_path.starts_with('/') {
        format!("{}{}", link.path.trim_end_matches('/'), file_path)
    } else {
        format!("{}/{}", link.path.trim_end_matches('/'), file_path)
    };

    let (_file_data, block_ids) =
        resolve_file_meta(state.db.as_ref(), &link.repo_id, &full_path).await?;

    increment_view_cnt(state.db.clone(), link.id);

    let filename = full_path
        .rsplit_once('/')
        .map(|(_, n)| n.to_string())
        .unwrap_or_else(|| full_path.clone());
    let stream = stream_blocks(block_ids, state.block_store.clone(), None);

    let mut headers = HeaderMap::new();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/octet-stream"),
    );
    headers.insert(
        HeaderName::from_static("content-disposition"),
        HeaderValue::from_str(&format!("attachment; filename=\"{}\"", filename))
            .unwrap_or_else(|_| HeaderValue::from_static("attachment")),
    );

    Ok((StatusCode::OK, headers, Body::from_stream(stream)).into_response())
}

// ── POST handler for directory share password ─────────────────────────

/// POST /d/{token}/ — validate password, redirect with password in URL.
pub async fn shared_dir_view_post(
    State(state): State<Arc<AppState>>,
    Path(token): Path<String>,
    axum::Form(form): axum::Form<HashMap<String, String>>,
) -> Result<Response, AppError> {
    let link = resolve_share_link(state.db.as_ref(), &token).await?;

    let password = form
        .get("password")
        .ok_or_else(|| AppError::BadRequest("password required".into()))?;

    let valid = crate::auth::password::verify_password(
        password,
        &link.password.unwrap_or_default(),
        state.config.auth.password_hash_iterations,
    );

    if !valid {
        let tpl = ShareAccessValidationTemplate {
            token: token.clone(),
            error: Some("Incorrect password".to_string()),
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
