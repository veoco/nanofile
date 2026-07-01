/// Web UI file browser handlers.
use askama::Template;
use axum::{
    extract::{Path, Query, State},
    http::{StatusCode, header},
    response::{Html, IntoResponse},
};
use sea_orm::{ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter};
use serde::Deserialize;
use std::collections::HashSet;
use std::sync::Arc;

use crate::AppState;
use crate::entity::{commit, repo};
use crate::error::AppError;
use crate::repo::download::Downloader;

use super::auth_extractor::WebUser;

// ─── Templates ───────────────────────────────────────────────────────────────

#[derive(Template)]
#[template(path = "files/browser.html")]
pub struct FileBrowserTemplate {
    pub urls: &'static crate::static_assets::TemplateUrls,
    pub user_email: String,
    pub is_admin: bool,
    pub csrf_token: String,
    pub repo_id: String,
    pub repo_name: String,
    pub current_path: String,
    pub breadcrumbs: Vec<BreadcrumbItem>,
    pub entries: Vec<FileEntry>,
    pub total: i64,
    pub has_more: bool,
    pub page: u32,
    /// "all" = render both views (full page), "list" = only list, "grid" = only grid
    pub render_view: &'static str,
    pub active_page: &'static str,
    pub left_panel_repos: Vec<crate::repo::LeftPanelRepo>,
    pub current_repo_id: Option<String>,
}

#[derive(Template)]
#[template(path = "files/browser_core.html")]
pub struct FileBrowserCoreTemplate {
    pub urls: &'static crate::static_assets::TemplateUrls,
    pub repo_name: String,
    pub repo_id: String,
    pub current_path: String,
    pub breadcrumbs: Vec<BreadcrumbItem>,
    pub entries: Vec<FileEntry>,
    pub total: i64,
    pub has_more: bool,
    pub page: u32,
    /// "all" = render both views (full page), "list" = only list, "grid" = only grid
    pub render_view: &'static str,
    pub csrf_token: String,
}

#[derive(Template)]
#[template(path = "files/preview_text.html")]
pub struct PreviewTextTemplate {
    pub urls: &'static crate::static_assets::TemplateUrls,
    pub user_email: String,
    pub is_admin: bool,
    pub repo_name: String,
    pub file_name: String,
    pub content: String,
    pub repo_id: String,
    pub current_path: String,
    pub parent_path: String,
    pub size_display: String,
    pub active_page: &'static str,
    pub left_panel_repos: Vec<crate::repo::LeftPanelRepo>,
    pub current_repo_id: Option<String>,
}

#[derive(Template)]
#[template(path = "files/preview_image.html")]
pub struct PreviewImageTemplate {
    pub urls: &'static crate::static_assets::TemplateUrls,
    pub user_email: String,
    pub is_admin: bool,
    pub repo_name: String,
    pub file_name: String,
    pub repo_id: String,
    pub current_path: String,
    pub parent_path: String,
    pub size_display: String,
    pub active_page: &'static str,
    pub left_panel_repos: Vec<crate::repo::LeftPanelRepo>,
    pub current_repo_id: Option<String>,
}

// ─── Data types ──────────────────────────────────────────────────────────────

#[derive(Clone)]
pub struct FileEntry {
    pub name: String,
    pub entry_type: String, // "file" or "dir"
    pub size: i64,
    pub size_display: String,
    pub mtime: i64,
    pub mtime_display: String,
    pub icon_color: &'static str,
    /// Relative path for use in URL construction, e.g. "Documents/file.txt"
    pub relative_path: String,
    /// Whether this file can be previewed inline (text/code/image).
    pub is_previewable: bool,
    /// Whether this file/directory is starred by the current user.
    pub starred: bool,
    /// File extension in uppercase (e.g. "PDF", "PNG"), None for directories.
    pub extension: Option<String>,
    /// Thumbnail URL for image files at list-view scale (48px), None otherwise.
    pub image_thumbnail_url: Option<String>,
    /// Thumbnail URL for image files at grid-view scale (256px), None otherwise.
    pub image_thumbnail_url_large: Option<String>,
}

/// Returns true if the file extension is one that the thumbnail service supports
/// for generating image thumbnails.
fn is_thumbnail_image(name: &str) -> bool {
    name.ends_with(".png")
        || name.ends_with(".jpg")
        || name.ends_with(".jpeg")
        || name.ends_with(".gif")
        || name.ends_with(".bmp")
        || name.ends_with(".webp")
}

pub fn is_previewable_file(name: &str) -> bool {
    // Images
    if name.ends_with(".png")
        || name.ends_with(".jpg")
        || name.ends_with(".jpeg")
        || name.ends_with(".gif")
        || name.ends_with(".webp")
        || name.ends_with(".bmp")
        || name.ends_with(".svg")
    {
        return true;
    }
    // Text / code
    name.ends_with(".txt")
        || name.ends_with(".md")
        || name.ends_with(".rs")
        || name.ends_with(".py")
        || name.ends_with(".js")
        || name.ends_with(".ts")
        || name.ends_with(".html")
        || name.ends_with(".css")
        || name.ends_with(".go")
        || name.ends_with(".java")
        || name.ends_with(".c")
        || name.ends_with(".cpp")
        || name.ends_with(".h")
        || name.ends_with(".rb")
        || name.ends_with(".php")
        || name.ends_with(".sh")
        || name.ends_with(".toml")
        || name.ends_with(".json")
        || name.ends_with(".yaml")
        || name.ends_with(".yml")
        || name.ends_with(".csv")
        || name.ends_with(".xml")
        || name.ends_with(".sql")
        || name.ends_with(".conf")
        || name.ends_with(".ini")
        || name.ends_with(".log")
}

fn file_icon_color(name: &str) -> &'static str {
    if name.ends_with(".png")
        || name.ends_with(".jpg")
        || name.ends_with(".jpeg")
        || name.ends_with(".gif")
        || name.ends_with(".webp")
        || name.ends_with(".bmp")
        || name.ends_with(".svg")
    {
        "text-purple-500"
    } else if name.ends_with(".rs")
        || name.ends_with(".py")
        || name.ends_with(".js")
        || name.ends_with(".ts")
        || name.ends_with(".html")
        || name.ends_with(".css")
        || name.ends_with(".go")
        || name.ends_with(".java")
        || name.ends_with(".c")
        || name.ends_with(".cpp")
        || name.ends_with(".h")
        || name.ends_with(".rb")
        || name.ends_with(".php")
        || name.ends_with(".sh")
        || name.ends_with(".toml")
        || name.ends_with(".json")
        || name.ends_with(".yaml")
        || name.ends_with(".yml")
    {
        "text-blue-500"
    } else if name.ends_with(".txt")
        || name.ends_with(".md")
        || name.ends_with(".pdf")
        || name.ends_with(".doc")
        || name.ends_with(".docx")
        || name.ends_with(".xlsx")
        || name.ends_with(".csv")
    {
        "text-green-500"
    } else if name.ends_with(".zip")
        || name.ends_with(".tar")
        || name.ends_with(".gz")
        || name.ends_with(".bz2")
        || name.ends_with(".7z")
        || name.ends_with(".rar")
        || name.ends_with(".zst")
    {
        "text-orange-500"
    } else {
        "text-gray-400"
    }
}

/// Extract the uppercase file extension from a name, or None for no extension.
fn file_extension(name: &str) -> Option<String> {
    let (_, ext) = name.rsplit_once('.')?;
    if ext.is_empty() || ext.contains('/') {
        return None;
    }
    Some(ext.to_uppercase())
}

pub fn format_size(bytes: i64) -> String {
    const KB: i64 = 1024;
    const MB: i64 = KB * 1024;
    const GB: i64 = MB * 1024;
    if bytes >= GB {
        format!("{:.1} GB", bytes as f64 / GB as f64)
    } else if bytes >= MB {
        format!("{:.1} MB", bytes as f64 / MB as f64)
    } else if bytes >= KB {
        format!("{:.1} KB", bytes as f64 / KB as f64)
    } else {
        format!("{} B", bytes)
    }
}

pub fn format_mtime(timestamp: i64) -> String {
    chrono::DateTime::from_timestamp(timestamp, 0)
        .map(|dt| dt.format("%Y-%m-%d %H:%M").to_string())
        .unwrap_or_else(|| timestamp.to_string())
}

#[derive(Clone)]
pub struct BreadcrumbItem {
    pub label: String,
    pub path: String,
}

// ─── Request types ───────────────────────────────────────────────────────────

#[derive(Deserialize)]
pub struct FileBrowserQuery {
    pub partial: Option<String>,
    pub dl: Option<String>,
    pub view: Option<String>,
    pub page: Option<u32>,
    pub per_page: Option<u32>,
}

// ─── Helpers ─────────────────────────────────────────────────────────────────

async fn verify_repo_access(
    db: &sea_orm::DatabaseConnection,
    user_id: i32,
    repo_id: &str,
) -> Result<(), AppError> {
    crate::storage::check_repo_read_permission(db, repo_id, user_id).await?;
    Ok(())
}

// ─── Handlers ────────────────────────────────────────────────────────────────

/// GET /library/{id}/{name}/ — repo file browser (root).
pub async fn file_browser_root(
    user: WebUser,
    State(state): State<Arc<AppState>>,
    Path(repo_id): Path<String>,
    Query(query): Query<FileBrowserQuery>,
) -> Result<impl IntoResponse, AppError> {
    file_browser_inner(user, state, repo_id, "/".to_string(), query).await
}

/// GET /library/{id}/{name}/{*path} — repo file browser (any path).
pub async fn file_browser(
    user: WebUser,
    State(state): State<Arc<AppState>>,
    Path((repo_id, path)): Path<(String, String)>,
    Query(query): Query<FileBrowserQuery>,
) -> Result<impl IntoResponse, AppError> {
    file_browser_inner(user, state, repo_id, normalize_path(&path), query).await
}

async fn file_browser_inner(
    user: WebUser,
    state: Arc<AppState>,
    repo_id: String,
    path: String,
    query: FileBrowserQuery,
) -> Result<impl IntoResponse, AppError> {
    let db = state.db.as_ref();
    let repos = &state.repos;
    verify_repo_access(db, user.user_id, &repo_id).await?;

    // Get repo name
    let repo_record = repos
        .repo
        .find_by_id(&repo_id)
        .await
        .map_err(|e| AppError::internal(format!("db error: {e}")))?
        .ok_or_else(|| AppError::NotFound("Repository not found".to_string()))?;

    // Try to list directory entries from the FS object tree.
    // If the path points to a file (not a directory), fall through to file serving.
    // For the root path `/`, treat errors as an empty directory (repo may be empty).
    let entries_result =
        crate::fs::handler::dir::list_dir_from_fs_tree(db, repos, &repo_id, &path).await;

    let entries_data = match entries_result {
        Ok(data) => data,
        Err(_e) if path == "/" => {
            // Root path listing failed → render empty directory (empty repo).
            (String::new(), vec![])
        }
        Err(AppError::NotFound(_)) => {
            // Path doesn't resolve as a directory — likely points to a file.
            // Fall through to file serving.
            return serve_file(user, state, repo_id, path, query)
                .await
                .map(IntoResponse::into_response);
        }
        Err(e) => {
            // Database errors, I/O errors — do NOT mask these as 500 Internal.
            return Err(e);
        }
    };

    // Query starred entries for this user+repo to stamp the `starred` field.
    let starred_set: HashSet<String> = repos
        .starred
        .find_by_user_and_repo(user.user_id, &repo_id)
        .await?
        .into_iter()
        .map(|s| s.path.trim_end_matches('/').to_string())
        .collect();

    let mut entries: Vec<FileEntry> = entries_data
        .1
        .into_iter()
        .map(|e| {
            let relative_path = if path == "/" {
                e.name.clone()
            } else {
                format!("{}/{}", path.trim_start_matches('/'), e.name)
            };
            let full_path = if path == "/" {
                format!("/{}", e.name)
            } else {
                format!("{}/{}", path.trim_end_matches('/'), e.name)
            };
            let is_previewable = is_previewable_file(&e.name);
            let ext = if e.entry_type == "file" {
                file_extension(&e.name)
            } else {
                None
            };
            let is_image_file = e.entry_type == "file" && is_thumbnail_image(&e.name);
            let thumb_url = if is_image_file {
                Some(format!(
                    "/api2/repos/{}/thumbnail/?p={}&size=48",
                    repo_id,
                    urlencode_path(&full_path)
                ))
            } else {
                None
            };
            let thumb_url_large = if is_image_file {
                Some(format!(
                    "/api2/repos/{}/thumbnail/?p={}&size=256",
                    repo_id,
                    urlencode_path(&full_path)
                ))
            } else {
                None
            };
            FileEntry {
                name: e.name.clone(),
                entry_type: e.entry_type,
                size: e.size,
                size_display: format_size(e.size),
                mtime: e.mtime,
                mtime_display: format_mtime(e.mtime),
                icon_color: file_icon_color(&e.name),
                relative_path,
                is_previewable,
                starred: starred_set.contains(&full_path),
                extension: ext,
                image_thumbnail_url: thumb_url,
                image_thumbnail_url_large: thumb_url_large,
            }
        })
        .collect();

    // Sort: directories first, then by name
    entries.sort_by(|a, b| {
        if a.entry_type != b.entry_type {
            if a.entry_type == "dir" {
                std::cmp::Ordering::Less
            } else {
                std::cmp::Ordering::Greater
            }
        } else {
            a.name.cmp(&b.name)
        }
    });

    // In-memory pagination (FS tree is content-addressed, can't SQL-paginate)
    let total = entries.len() as i64;
    let per_page = query.per_page.unwrap_or(200).min(500) as usize;
    let page = query.page.unwrap_or(1).max(1) as usize;
    let offset = (page - 1) * per_page;
    let has_more = offset + per_page < total as usize;
    entries = if offset < entries.len() {
        let end = (offset + per_page).min(entries.len());
        entries[offset..end].to_vec()
    } else {
        vec![]
    };

    // Build breadcrumb items from current_path.
    // Each item's path is relative (no leading /) for use in URL construction.
    let mut breadcrumbs: Vec<BreadcrumbItem> = Vec::new();
    if path != "/" {
        let trimmed = path.trim_start_matches('/');
        let segments: Vec<&str> = trimmed.split('/').filter(|s| !s.is_empty()).collect();
        let mut accum = String::new();
        for seg in &segments {
            if !accum.is_empty() {
                accum.push('/');
            }
            accum.push_str(seg);
            breadcrumbs.push(BreadcrumbItem {
                label: seg.to_string(),
                path: accum.clone(),
            });
        }
    }

    let is_partial = query.partial.as_deref() == Some("1");

    let csrf_token =
        crate::auth::csrf::generate_csrf_token(&state.csrf_secret, &user.session_token);

    // Determine which view(s) to render
    let render_view = match query.view.as_deref() {
        Some("list") => "list",
        Some("grid") => "grid",
        _ => "all",
    };

    if is_partial {
        let tpl = FileBrowserCoreTemplate {
            urls: crate::static_assets::template_urls(),
            repo_name: repo_record.name.clone(),
            repo_id: repo_id.clone(),
            current_path: path.clone(),
            breadcrumbs: breadcrumbs.clone(),
            entries,
            total,
            has_more,
            page: page as u32,
            render_view,
            csrf_token,
        };
        let html = tpl
            .render()
            .map_err(|e| AppError::internal(e.to_string()))?;
        Ok(Html(html).into_response())
    } else {
        let left_panel_repos =
            crate::repo::load_left_panel_repos(state.db.as_ref(), user.user_id).await?;
        let current_repo_id = Some(repo_id.clone());
        let tpl = FileBrowserTemplate {
            urls: crate::static_assets::template_urls(),
            user_email: user.email,
            is_admin: user.is_admin,
            csrf_token,
            repo_id,
            repo_name: repo_record.name,
            current_path: path,
            breadcrumbs,
            entries,
            total,
            has_more,
            page: page as u32,
            render_view: "all",
            active_page: "repos",
            left_panel_repos,
            current_repo_id,
        };
        let html = tpl
            .render()
            .map_err(|e| AppError::internal(e.to_string()))?;
        Ok(Html(html).into_response())
    }
}

/// Serve a file directly from the repo (preview or download).
/// Called by `file_browser_inner` when the path points to a file, not a directory.
async fn serve_file(
    user: WebUser,
    state: Arc<AppState>,
    repo_id: String,
    path: String,
    query: FileBrowserQuery,
) -> Result<impl IntoResponse, AppError> {
    let db = state.db.as_ref();
    let path = normalize_path(&path);
    let file_name = path.rsplit('/').next().unwrap_or("file").to_string();

    // ?dl=1 → force download
    if query.dl.as_deref() == Some("1") {
        let data = Downloader::download_file(db, &repo_id, &path, &state.block_store, None)
            .await
            .map_err(|e| AppError::Internal(format!("download failed: {e}")))?;
        let content_type = mime_guess(&file_name);
        let disposition = format!("attachment; filename=\"{}\"", file_name);
        return Ok((
            StatusCode::OK,
            [
                (header::CONTENT_TYPE, content_type),
                (header::CONTENT_DISPOSITION, &disposition),
            ],
            data,
        )
            .into_response());
    }

    // Image preview
    let is_image = file_name.ends_with(".png")
        || file_name.ends_with(".jpg")
        || file_name.ends_with(".jpeg")
        || file_name.ends_with(".gif")
        || file_name.ends_with(".webp")
        || file_name.ends_with(".bmp")
        || file_name.ends_with(".svg");

    // Text/code preview
    let is_text = is_previewable_file(&file_name);

    if is_image {
        let size_display = get_file_size(db, &repo_id, &path)
            .await
            .map(format_size)
            .unwrap_or_else(|_| "?".to_string());

        let repo_name = state
            .repos
            .repo
            .find_by_id(&repo_id)
            .await
            .map_err(|e| AppError::internal(format!("db error: {e}")))?
            .ok_or_else(|| AppError::NotFound("Repository not found".to_string()))?
            .name;

        let raw_parent = parent_path_from(&path);
        let parent_path = raw_parent.trim_start_matches('/').to_string();

        let left_panel_repos =
            crate::repo::load_left_panel_repos(state.db.as_ref(), user.user_id).await?;
        let tpl = PreviewImageTemplate {
            urls: crate::static_assets::template_urls(),
            user_email: user.email,
            is_admin: user.is_admin,
            repo_name,
            file_name,
            repo_id: repo_id.clone(),
            current_path: path.trim_start_matches('/').to_string(),
            parent_path,
            size_display,
            active_page: "repos",
            left_panel_repos,
            current_repo_id: Some(repo_id),
        };
        let html = tpl
            .render()
            .map_err(|e| AppError::internal(e.to_string()))?;
        return Ok(Html(html).into_response());
    }

    if is_text {
        let data = Downloader::download_file(db, &repo_id, &path, &state.block_store, None)
            .await
            .map_err(|e| AppError::Internal(format!("download failed: {e}")))?;
        let content = String::from_utf8_lossy(&data).to_string();

        let repo_name = state
            .repos
            .repo
            .find_by_id(&repo_id)
            .await
            .map_err(|e| AppError::internal(format!("db error: {e}")))?
            .ok_or_else(|| AppError::NotFound("Repository not found".to_string()))?
            .name;

        let raw_parent = parent_path_from(&path);
        let parent_path = raw_parent.trim_start_matches('/').to_string();

        let size_display = get_file_size(db, &repo_id, &path)
            .await
            .map(format_size)
            .unwrap_or_else(|_| "?".to_string());

        let left_panel_repos =
            crate::repo::load_left_panel_repos(state.db.as_ref(), user.user_id).await?;
        let tpl = PreviewTextTemplate {
            urls: crate::static_assets::template_urls(),
            user_email: user.email,
            is_admin: user.is_admin,
            repo_name,
            file_name,
            content,
            repo_id: repo_id.clone(),
            current_path: path.trim_start_matches('/').to_string(),
            parent_path,
            size_display,
            active_page: "repos",
            left_panel_repos,
            current_repo_id: Some(repo_id),
        };
        let html = tpl
            .render()
            .map_err(|e| AppError::internal(e.to_string()))?;
        return Ok(Html(html).into_response());
    }

    // Binary files — serve raw bytes inline
    let data = Downloader::download_file(db, &repo_id, &path, &state.block_store, None)
        .await
        .map_err(|e| AppError::Internal(format!("download failed: {e}")))?;
    let content_type = mime_guess(&file_name);
    Ok((StatusCode::OK, [(header::CONTENT_TYPE, content_type)], data).into_response())
}

/// Resolve a file's size from the FS tree without downloading its content.
async fn get_file_size(
    db: &sea_orm::DatabaseConnection,
    repo_id: &str,
    path: &str,
) -> Result<i64, AppError> {
    let head_root_id = get_head_root_id(db, repo_id).await?;
    let parent_path = parent_path_from(path);
    let file_name = path.rsplit_once('/').map(|(_, n)| n).unwrap_or("");

    if parent_path == "/" {
        // Root-level file: resolve from root's directory listing
        let dir_data = crate::repo::read_fs_dir_data(db, repo_id, &head_root_id)
            .await
            .map_err(|e| AppError::Internal(format!("read parent failed: {e}")))?;
        return dir_data
            .dirents
            .iter()
            .find(|d| d.name == file_name)
            .map(|d| d.size)
            .ok_or_else(|| AppError::NotFound("File not found".to_string()));
    }

    let parent_fs_id = crate::repo::resolve_fs_id(db, repo_id, &head_root_id, parent_path)
        .await
        .map_err(|e| AppError::Internal(format!("resolve parent failed: {e}")))?;

    let dir_data = crate::repo::read_fs_dir_data(db, repo_id, &parent_fs_id)
        .await
        .map_err(|e| AppError::Internal(format!("read parent failed: {e}")))?;
    dir_data
        .dirents
        .iter()
        .find(|d| d.name == file_name)
        .map(|d| d.size)
        .ok_or_else(|| AppError::NotFound("File not found".to_string()))
}

// ─── Utilities ───────────────────────────────────────────────────────────────

/// Get the root fs_id from the repo's head commit for path resolution.
async fn get_head_root_id(db: &DatabaseConnection, repo_id: &str) -> Result<String, AppError> {
    let repo_record = repo::Entity::find_by_id(repo_id)
        .one(db)
        .await
        .map_err(|e| AppError::Internal(format!("db error: {e}")))?
        .ok_or_else(|| AppError::NotFound("Repository not found".to_string()))?;

    let head_commit_id = repo_record
        .head_commit_id
        .ok_or_else(|| AppError::NotFound("No commits yet".to_string()))?;

    let head = commit::Entity::find()
        .filter(commit::Column::RepoId.eq(repo_id))
        .filter(commit::Column::CommitId.eq(&head_commit_id))
        .one(db)
        .await
        .map_err(|e| AppError::Internal(format!("db error: {e}")))?
        .ok_or_else(|| AppError::NotFound("Head commit not found".to_string()))?;

    Ok(head.root_id)
}

/// Extract the parent directory path from a full path.
/// `/dir/file.txt` → `/dir`,  `/file.txt` → `/`
fn parent_path_from(path: &str) -> &str {
    match path.rsplit_once('/') {
        Some(("", _)) => "/",
        Some((parent, _)) => parent,
        None => "/",
    }
}

fn normalize_path(path: &str) -> String {
    if path.is_empty() || path == "/" {
        "/".to_string()
    } else if path.starts_with('/') {
        path.to_string()
    } else {
        format!("/{}", path)
    }
}

fn mime_guess(filename: &str) -> &'static str {
    if filename.ends_with(".txt")
        || filename.ends_with(".md")
        || filename.ends_with(".rs")
        || filename.ends_with(".py")
        || filename.ends_with(".js")
        || filename.ends_with(".html")
        || filename.ends_with(".css")
        || filename.ends_with(".json")
        || filename.ends_with(".toml")
        || filename.ends_with(".yaml")
        || filename.ends_with(".yml")
    {
        "text/plain; charset=utf-8"
    } else if filename.ends_with(".png") {
        "image/png"
    } else if filename.ends_with(".jpg") || filename.ends_with(".jpeg") {
        "image/jpeg"
    } else if filename.ends_with(".gif") {
        "image/gif"
    } else if filename.ends_with(".pdf") {
        "application/pdf"
    } else {
        "application/octet-stream"
    }
}

/// Percent-encode a URL path segment for use in query parameters.
fn urlencode_path(path: &str) -> String {
    // Encode everything except unreserved characters (RFC 3986)
    percent_encoding::utf8_percent_encode(path, percent_encoding::NON_ALPHANUMERIC).to_string()
}
