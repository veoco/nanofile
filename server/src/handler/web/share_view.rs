use askama::Template;
use axum::{
    body::Body,
    extract::{Path, Query, State},
    http::{HeaderMap, HeaderName, HeaderValue, StatusCode, header},
    response::{Html, IntoResponse, Response},
};
use chrono::TimeZone;
use futures::{Stream, StreamExt};
use std::collections::HashMap;
use std::sync::Arc;

use crate::AppState;
use crate::fs::core::download::Downloader;
use crate::fs::core::tree::{read_fs_dir_data, read_fs_file_data, resolve_fs_id};
use base::common::FsFileData;
use base::error::AppError;
use infra::common::{EMPTY_SHA1, S_IFDIR};

use async_zip::tokio::write::ZipFileWriter;
use async_zip::{Compression, ZipEntryBuilder};
use futures::io::AsyncWriteExt;
use tokio_util::io::ReaderStream;

// ── Stream blocks helper (copied from download.rs) ────────────────────────

fn stream_blocks(
    block_ids: Vec<String>,
    block_store: infra::storage::DynBlockStorage,
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
                Some((k, iv)) => infra::crypto::random_key::decrypt_block(&data, k, iv)
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
    pub created_at_display: String,
    pub download_url: String,
    pub description: Option<String>,
}

#[allow(dead_code)]
#[derive(Template)]
#[template(path = "web/share_access_validation.html")]
struct ShareAccessValidationTemplate {
    pub token: String,
    pub error: Option<String>,
    pub form_action: String,
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
            form_action: format!("/f/{}/", token),
        };
        let html = tpl
            .render()
            .map_err(|e| AppError::Internal(e.to_string()))?;
        return Ok(Html(html).into_response());
    }

    // Handle ?dl=1 — download the file directly
    if params.get("dl").map(|s| s.as_str()) == Some("1") {
        let (_file_data, block_ids) =
            resolve_file_meta(&state.repos, &link.repo_id, &link.path).await?;

        let filename = link
            .path
            .rsplit_once('/')
            .map(|(_, n)| n)
            .unwrap_or(&link.path);
        let stream = stream_blocks(block_ids, state.block_store.clone(), None);

        crate::service::sharing::share::increment_view_cnt(state.repos.share_link.clone(), link.id);

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
        created_at_display: created_display,
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
    Path(token): Path<String>,
    axum::Form(form): axum::Form<HashMap<String, String>>,
) -> Result<Response, AppError> {
    let link = crate::service::sharing::share::resolve_share_link(&state.repos, &token).await?;

    let password = form
        .get("password")
        .ok_or_else(|| AppError::BadRequest("password required".into()))?;

    let valid = crate::service::auth::password::verify_password(
        password,
        &link.password.unwrap_or_default(),
        state.config.auth.password_hash_iterations,
    );

    if !valid {
        // Show password form again with error
        let tpl = ShareAccessValidationTemplate {
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
    pub token: String,
    pub dir_name: String,
    pub dir_path: String,
    pub parent_path: Option<String>,
    pub entries: Vec<DirEntryInfo>,
    pub item_count: usize,
    pub has_password: bool,
    pub expires_at_display: String,
    pub created_at_display: String,
    pub download_url: String,
    pub password_query: String,
    pub description: Option<String>,
}

struct DirEntryInfo {
    pub name: String,
    pub ext: String,
    pub is_dir: bool,
    pub size: String,
    pub full_path: String,
}

/// Recursively collect all files under a directory for ZIP streaming.
#[allow(dead_code)]
struct ZipEntry {
    path_in_zip: String,
    block_ids: Vec<String>,
    size: i64,
}

async fn collect_zip_entries(
    repos: &crate::repository::Repositories,
    repo_id: &str,
    root_fs_id: &str,
    dir_path: &str,
    zip_prefix: &str,
) -> Result<Vec<ZipEntry>, AppError> {
    let dir_id = if dir_path == "/" {
        root_fs_id.to_string()
    } else {
        resolve_fs_id(repos, repo_id, root_fs_id, dir_path)
            .await
            .map_err(|_| AppError::NotFound("Directory not found".into()))?
    };

    let mut entries = Vec::new();
    let mut stack: Vec<(String, String)> = vec![(dir_id, zip_prefix.to_string())];

    while let Some((fs_id, prefix)) = stack.pop() {
        if fs_id == EMPTY_SHA1 {
            continue;
        }
        let dir_data = match read_fs_dir_data(repos, repo_id, &fs_id).await {
            Ok(d) => d,
            Err(_) => continue,
        };
        for dirent in &dir_data.dirents {
            let is_dir = dirent.mode & S_IFDIR != 0;
            let entry_path = if prefix.is_empty() {
                dirent.name.clone()
            } else {
                format!("{}/{}", prefix, dirent.name)
            };
            if is_dir {
                stack.push((dirent.id.clone(), entry_path));
            } else {
                let file_data = match read_fs_file_data(repos, repo_id, &dirent.id).await {
                    Ok(f) => f,
                    Err(_) => continue,
                };
                entries.push(ZipEntry {
                    path_in_zip: entry_path,
                    block_ids: file_data.block_ids,
                    size: file_data.size,
                });
            }
        }
    }
    Ok(entries)
}

/// Stream a ZIP archive over HTTP using async_zip duplex.
fn stream_zip(
    block_store: infra::storage::DynBlockStorage,
    files: Vec<ZipEntry>,
) -> impl futures::Stream<Item = Result<bytes::Bytes, std::io::Error>> {
    let (duplex_writer, duplex_reader) = tokio::io::duplex(64 * 1024);

    tokio::spawn(async move {
        let mut zip = ZipFileWriter::with_tokio(duplex_writer);
        for entry in &files {
            let builder =
                ZipEntryBuilder::new(entry.path_in_zip.clone().into(), Compression::Deflate);
            let mut entry_writer = zip
                .write_entry_stream(builder)
                .await
                .map_err(|e| std::io::Error::other(e.to_string()))?;
            for block_id in &entry.block_ids {
                let data = block_store.read_block(block_id).await?;
                entry_writer.write_all(&data).await?;
            }
            entry_writer
                .close()
                .await
                .map_err(|e| std::io::Error::other(e.to_string()))?;
        }
        zip.close()
            .await
            .map_err(|e| std::io::Error::other(e.to_string()))?;
        Ok::<(), std::io::Error>(())
    });

    ReaderStream::new(duplex_reader)
}

/// GET /d/{token}/ — show directory file listing, or ?dl=1 to download ZIP.
pub async fn shared_dir_view(
    State(state): State<Arc<AppState>>,
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
            form_action: format!("/f/{}/", token),
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
        let files = collect_zip_entries(
            &state.repos,
            &link.repo_id,
            &head_commit.root_id,
            &link.path,
            &dir_name,
        )
        .await?;

        crate::service::sharing::share::increment_view_cnt(state.repos.share_link.clone(), link.id);
        let stream = stream_zip(state.block_store.clone(), files);

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

    // Get repo head commit and resolve directory
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

    let item_count = entries.len();

    let expires_display = match link.expires_at {
        Some(ts) => format_timestamp(ts),
        None => "Never".to_string(),
    };
    let created_display = format_timestamp(link.created_at);

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
        token: link.token.clone(),
        dir_name,
        dir_path: sub_path.to_string(),
        parent_path,
        entries,
        item_count,
        has_password: link.password.is_some(),
        expires_at_display: expires_display,
        created_at_display: created_display,
        download_url,
        password_query: pw_query,
        description: link.description.clone(),
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
        resolve_file_meta(&state.repos, &link.repo_id, &full_path).await?;

    crate::service::sharing::share::increment_view_cnt(state.repos.share_link.clone(), link.id);

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
    let link = crate::service::sharing::share::resolve_share_link(&state.repos, &token).await?;

    let password = form
        .get("password")
        .ok_or_else(|| AppError::BadRequest("password required".into()))?;

    let valid = crate::service::auth::password::verify_password(
        password,
        &link.password.unwrap_or_default(),
        state.config.auth.password_hash_iterations,
    );

    if !valid {
        let tpl = ShareAccessValidationTemplate {
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
