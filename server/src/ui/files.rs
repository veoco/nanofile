/// Web UI file browser handlers.
use askama::Template;
use axum::{
    extract::{Path, Query, State},
    http::{StatusCode, header},
    response::{Html, IntoResponse},
};
use serde::Deserialize;
use std::collections::HashSet;
use std::sync::Arc;

use crate::AppState;
use crate::fs::core::download::Downloader;
use base::error::AppError;
use infra::common::util::parent_path_from;

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
    pub left_panel_repos: Vec<crate::service::repo::service::LeftPanelRepo>,
    pub current_repo_id: Option<String>,
    /// Maximum upload file size in MB, from server config.
    pub max_upload_size_mb: u64,
    pub sort_field: String,
    pub sort_order: String,
    pub gallery_groups: Vec<GalleryMonthGroup>,
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
    pub sort_field: String,
    pub sort_order: String,
    pub gallery_groups: Vec<GalleryMonthGroup>,
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
    pub left_panel_repos: Vec<crate::service::repo::service::LeftPanelRepo>,
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
    pub left_panel_repos: Vec<crate::service::repo::service::LeftPanelRepo>,
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
    /// Whether this file is a video (used for gallery view rendering).
    pub is_video: bool,
    /// Email of the user who last modified this entry.
    pub modifier_email: String,
}

/// A group of file entries belonging to the same calendar month, used by gallery view.
#[derive(Clone)]
pub struct GalleryMonthGroup {
    /// Month label like "June 2026"
    pub label: String,
    /// Entries belonging to this month, sorted by mtime descending.
    pub entries: Vec<FileEntry>,
}

/// Returns true if the file extension indicates a video file.
/// Used by gallery view to render video placeholders with play icon.
pub fn is_video_file(name: &str) -> bool {
    std::path::Path::new(name)
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| {
            matches!(
                e.to_ascii_lowercase().as_str(),
                "mp4" | "mov" | "avi" | "mkv" | "webm" | "wmv" | "flv" | "3gp"
            )
        })
        .unwrap_or(false)
}

/// Format a unix timestamp into a month label like "June 2026".
pub fn format_month_label(timestamp: i64) -> String {
    chrono::DateTime::from_timestamp(timestamp, 0)
        .map(|dt| dt.format("%B %Y").to_string())
        .unwrap_or_else(|| "Unknown".to_string())
}

/// Group already-sorted (by mtime descending) entries by calendar month.
/// Returns groups in descending month order (newest first).
pub fn group_entries_by_month(entries: Vec<FileEntry>) -> Vec<GalleryMonthGroup> {
    let mut groups: Vec<GalleryMonthGroup> = Vec::new();
    for entry in entries {
        let label = format_month_label(entry.mtime);
        if groups.last().map(|g| g.label.as_str()) != Some(label.as_str()) {
            groups.push(GalleryMonthGroup {
                label,
                entries: Vec::new(),
            });
        }
        groups.last_mut().unwrap().entries.push(entry);
    }
    groups
}

/// Sort file entries: directories always first, then by the specified field and order.
/// Default field is "name", default order is "asc".
pub fn sort_entries(entries: &mut [FileEntry], sort: &str, sort_order: &str) {
    entries.sort_by(|a, b| {
        // Dirs always before files
        if a.entry_type != b.entry_type {
            return if a.entry_type == "dir" {
                std::cmp::Ordering::Less
            } else {
                std::cmp::Ordering::Greater
            };
        }
        let cmp = match sort {
            "mtime" => a.mtime.cmp(&b.mtime),
            "size" => a.size.cmp(&b.size),
            _ => a.name.to_lowercase().cmp(&b.name.to_lowercase()),
        };
        if sort_order == "desc" {
            cmp.reverse()
        } else {
            cmp
        }
    });
}

/// Returns true if the file extension is one that the thumbnail service supports
/// for generating image thumbnails.
fn is_thumbnail_image(name: &str) -> bool {
    std::path::Path::new(name)
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| crate::thumbnail_util::is_supported_image_ext(&e.to_lowercase()))
        .unwrap_or(false)
}

pub fn is_previewable_file(name: &str) -> bool {
    let name = &name.to_ascii_lowercase();
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
    pub sort: Option<String>,       // "name" | "mtime" | "size"
    pub sort_order: Option<String>, // "asc" | "desc"
}

// ─── Helpers ─────────────────────────────────────────────────────────────────

async fn verify_repo_access(
    db: &sea_orm::DatabaseConnection,
    user_id: i32,
    repo_id: &str,
) -> Result<(), AppError> {
    crate::domain::permission::check_repo_read_permission(db, repo_id, user_id).await?;
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
    let path = base::sanitize::safe_normalize_path(&path)
        .map_err(|e| AppError::BadRequest(format!("Invalid path: {e}")))?;
    file_browser_inner(user, state, repo_id, path, query).await
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
        .await?
        .ok_or_else(|| AppError::NotFound("Repository not found".to_string()))?;

    // Try to list directory entries from the FS object tree.
    // If the path points to a file (not a directory), fall through to file serving.
    // For the root path `/`, treat errors as an empty directory (repo may be empty).
    let entries_result =
        crate::service::fs::dir::list_dir_from_fs_tree(db, repos, &repo_id, &path).await;

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
            let entry_is_video = e.entry_type == "file" && is_video_file(&e.name);
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
                is_video: entry_is_video,
                modifier_email: e.modifier.clone(),
            }
        })
        .collect();

    // Sort: directories first, then by configurable field and order
    let sort_field = query.sort.as_deref().unwrap_or("name");
    let sort_order = query.sort_order.as_deref().unwrap_or("asc");
    sort_entries(&mut entries, sort_field, sort_order);

    // In-memory pagination (FS tree is content-addressed, can't SQL-paginate)
    let total = entries.len() as i64;
    let per_page = query.per_page.unwrap_or(200).min(500) as usize;
    let page = query.page.unwrap_or(1).max(1) as usize;
    let offset = (page - 1) * per_page;
    let has_more = offset + per_page < total as usize;

    // Determine view mode after pagination so gallery can use the same page slice
    let render_view = match query.view.as_deref() {
        Some("list") => "list",
        Some("grid") => "grid",
        Some("gallery") => "gallery",
        _ => "all",
    };

    // Gallery groups: built from ALL entries sorted by mtime desc, independently paginated.
    // This ensures gallery maintains correct reverse-chronological order regardless
    // of the configured sort used by list/grid views.
    let mut gallery_media: Vec<FileEntry> = Vec::new();
    let gallery_total: i64;
    if render_view == "gallery" || render_view == "all" {
        let mut media: Vec<FileEntry> = entries
            .iter()
            .filter(|e| e.entry_type == "file" && (e.is_video || e.image_thumbnail_url.is_some()))
            .cloned()
            .collect();
        media.sort_by_key(|b| std::cmp::Reverse(b.mtime)); // mtime descending
        gallery_total = media.len() as i64;
        gallery_media = media;
    } else {
        gallery_total = 0;
    }

    // Now paginate entries for list/grid views
    entries = if offset < entries.len() {
        let end = (offset + per_page).min(entries.len());
        entries[offset..end].to_vec()
    } else {
        vec![]
    };

    // Build gallery month groups from the mtime-desc sorted media (paginated independently)
    let gallery_groups: Vec<GalleryMonthGroup> = if render_view == "gallery" || render_view == "all"
    {
        let gallery_offset = (page - 1) * per_page;
        let paginated: Vec<FileEntry> = if gallery_offset < gallery_media.len() {
            let end = (gallery_offset + per_page).min(gallery_media.len());
            gallery_media[gallery_offset..end].to_vec()
        } else {
            vec![]
        };
        group_entries_by_month(paginated)
    } else {
        vec![]
    };

    // In gallery-only mode, override pagination info to reflect media counts
    let (effective_total, effective_has_more) = if render_view == "gallery" {
        (gallery_total, page * per_page < gallery_total as usize)
    } else {
        (total, has_more)
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
        crate::service::auth::csrf::generate_csrf_token(&state.csrf_secret, &user.session_token);

    if is_partial {
        let tpl = FileBrowserCoreTemplate {
            urls: crate::static_assets::template_urls(),
            repo_name: repo_record.name.clone(),
            repo_id: repo_id.clone(),
            current_path: path.clone(),
            breadcrumbs: breadcrumbs.clone(),
            entries,
            total: effective_total,
            has_more: effective_has_more,
            page: page as u32,
            render_view,
            csrf_token,
            sort_field: sort_field.to_string(),
            sort_order: sort_order.to_string(),
            gallery_groups,
        };
        let html = tpl
            .render()
            .map_err(|e| AppError::internal(e.to_string()))?;
        Ok(Html(html).into_response())
    } else {
        let left_panel_repos =
            crate::service::repo::service::load_left_panel_repos(&state.repos, user.user_id)
                .await?;
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
            max_upload_size_mb: state.config.server.max_upload_size_mb,
            sort_field: sort_field.to_string(),
            sort_order: sort_order.to_string(),
            gallery_groups,
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
    let path = base::sanitize::safe_normalize_path(&path)
        .map_err(|e| AppError::BadRequest(format!("Invalid path: {e}")))?;
    let file_name = path.rsplit('/').next().unwrap_or("file").to_string();

    // ?dl=1 → force download
    if query.dl.as_deref() == Some("1") {
        let data =
            Downloader::download_file(&state.repos, db, &repo_id, &path, &state.block_store, None)
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
        let size_display = get_file_size(&state.repos, &repo_id, &path)
            .await
            .map(format_size)
            .unwrap_or_else(|_| "?".to_string());

        let repo_name = state
            .repos
            .repo
            .find_by_id(&repo_id)
            .await?
            .ok_or_else(|| AppError::NotFound("Repository not found".to_string()))?
            .name;

        let raw_parent = parent_path_from(&path);
        let parent_path = raw_parent.trim_start_matches('/').to_string();

        let left_panel_repos =
            crate::service::repo::service::load_left_panel_repos(&state.repos, user.user_id)
                .await?;
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
        let data =
            Downloader::download_file(&state.repos, db, &repo_id, &path, &state.block_store, None)
                .await
                .map_err(|e| AppError::Internal(format!("download failed: {e}")))?;
        let content = String::from_utf8_lossy(&data).to_string();

        let repo_name = state
            .repos
            .repo
            .find_by_id(&repo_id)
            .await?
            .ok_or_else(|| AppError::NotFound("Repository not found".to_string()))?
            .name;

        let raw_parent = parent_path_from(&path);
        let parent_path = raw_parent.trim_start_matches('/').to_string();

        let size_display = get_file_size(&state.repos, &repo_id, &path)
            .await
            .map(format_size)
            .unwrap_or_else(|_| "?".to_string());

        let left_panel_repos =
            crate::service::repo::service::load_left_panel_repos(&state.repos, user.user_id)
                .await?;
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
    let data =
        Downloader::download_file(&state.repos, db, &repo_id, &path, &state.block_store, None)
            .await
            .map_err(|e| AppError::Internal(format!("download failed: {e}")))?;
    let content_type = mime_guess(&file_name);
    Ok((StatusCode::OK, [(header::CONTENT_TYPE, content_type)], data).into_response())
}

/// Resolve a file's size from the FS tree without downloading its content.
async fn get_file_size(
    repos: &crate::repository::Repositories,
    repo_id: &str,
    path: &str,
) -> Result<i64, AppError> {
    let head_root_id = get_head_root_id(repos, repo_id).await?;
    let parent_path = parent_path_from(path);
    let file_name = path.rsplit_once('/').map(|(_, n)| n).unwrap_or("");

    if parent_path == "/" {
        // Root-level file: resolve from root's directory listing
        let dir_data = crate::fs::core::read_fs_dir_data(repos, repo_id, &head_root_id)
            .await
            .map_err(|e| AppError::Internal(format!("read parent failed: {e}")))?;
        return dir_data
            .dirents
            .iter()
            .find(|d| d.name == file_name)
            .map(|d| d.size)
            .ok_or_else(|| AppError::NotFound("File not found".to_string()));
    }

    let parent_fs_id = crate::fs::core::resolve_fs_id(repos, repo_id, &head_root_id, parent_path)
        .await
        .map_err(|e| AppError::Internal(format!("resolve parent failed: {e}")))?;

    let dir_data = crate::fs::core::read_fs_dir_data(repos, repo_id, &parent_fs_id)
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
async fn get_head_root_id(
    repos: &crate::repository::Repositories,
    repo_id: &str,
) -> Result<String, AppError> {
    let repo_record = repos
        .repo
        .find_by_id(repo_id)
        .await?
        .ok_or_else(|| AppError::NotFound("Repository not found".to_string()))?;

    let head_commit_id = repo_record
        .head_commit_id
        .ok_or_else(|| AppError::NotFound("No commits yet".to_string()))?;

    let head = repos
        .commit
        .find_by_repo_and_commit_id(repo_id, &head_commit_id)
        .await?
        .ok_or_else(|| AppError::NotFound("Head commit not found".to_string()))?;

    Ok(head.root_id)
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

#[cfg(test)]
mod tests {
    use super::*;

    fn make_entry(name: &str, entry_type: &str, size: i64, mtime: i64) -> FileEntry {
        FileEntry {
            name: name.to_string(),
            entry_type: entry_type.to_string(),
            size,
            size_display: String::new(),
            mtime,
            mtime_display: String::new(),
            icon_color: "",
            relative_path: String::new(),
            is_previewable: false,
            starred: false,
            extension: None,
            image_thumbnail_url: None,
            image_thumbnail_url_large: None,
            is_video: false,
            modifier_email: String::new(),
        }
    }

    #[test]
    fn test_sort_default_name_asc() {
        let mut entries = vec![
            make_entry("b", "file", 0, 3),
            make_entry("a", "dir", 0, 2),
            make_entry("c", "file", 0, 1),
            make_entry("d", "dir", 0, 4),
        ];
        sort_entries(&mut entries, "name", "asc");
        assert_eq!(entries[0].name, "a"); // dir first
        assert_eq!(entries[1].name, "d"); // dir second
        assert_eq!(entries[2].name, "b"); // file
        assert_eq!(entries[3].name, "c");
    }

    #[test]
    fn test_sort_name_desc() {
        let mut entries = vec![
            make_entry("b", "file", 0, 0),
            make_entry("a", "file", 0, 0),
            make_entry("c", "dir", 0, 0),
        ];
        sort_entries(&mut entries, "name", "desc");
        assert_eq!(entries[0].name, "c"); // dir first
        assert_eq!(entries[1].name, "b"); // files: desc order
        assert_eq!(entries[2].name, "a");
    }

    #[test]
    fn test_sort_mtime_asc() {
        let mut entries = vec![
            make_entry("old", "file", 0, 10),
            make_entry("new", "file", 0, 100),
            make_entry("dir1", "dir", 0, 50),
        ];
        sort_entries(&mut entries, "mtime", "asc");
        assert_eq!(entries[0].name, "dir1");
        assert_eq!(entries[1].name, "old"); // file with mtime=10
        assert_eq!(entries[2].name, "new"); // file with mtime=100
    }

    #[test]
    fn test_sort_mtime_desc() {
        let mut entries = vec![
            make_entry("old", "file", 0, 10),
            make_entry("new", "file", 0, 100),
            make_entry("dir1", "dir", 0, 50),
        ];
        sort_entries(&mut entries, "mtime", "desc");
        assert_eq!(entries[0].name, "dir1");
        assert_eq!(entries[1].name, "new"); // file with mtime=100
        assert_eq!(entries[2].name, "old"); // file with mtime=10
    }

    #[test]
    fn test_sort_size_asc() {
        let mut entries = vec![
            make_entry("big", "file", 1000, 0),
            make_entry("small", "file", 10, 0),
            make_entry("dir1", "dir", 999, 0),
        ];
        sort_entries(&mut entries, "size", "asc");
        assert_eq!(entries[0].name, "dir1");
        assert_eq!(entries[1].name, "small");
        assert_eq!(entries[2].name, "big");
    }

    #[test]
    fn test_sort_size_desc() {
        let mut entries = vec![
            make_entry("big", "file", 1000, 0),
            make_entry("small", "file", 10, 0),
            make_entry("dir1", "dir", 999, 0),
        ];
        sort_entries(&mut entries, "size", "desc");
        assert_eq!(entries[0].name, "dir1");
        assert_eq!(entries[1].name, "big");
        assert_eq!(entries[2].name, "small");
    }

    #[test]
    fn test_sort_dirs_always_first() {
        let mut entries = vec![
            make_entry("z_file", "file", 0, 0),
            make_entry("a_dir", "dir", 0, 0),
            make_entry("m_dir", "dir", 0, 0),
        ];
        sort_entries(&mut entries, "name", "asc");
        assert_eq!(entries[0].name, "a_dir");
        assert_eq!(entries[1].name, "m_dir");
        assert_eq!(entries[2].name, "z_file");

        // Also verify with mtime sort
        sort_entries(&mut entries, "mtime", "desc");
        assert_eq!(entries[0].entry_type, "dir");
        assert_eq!(entries[1].entry_type, "dir");
        assert_eq!(entries[2].entry_type, "file");
    }

    #[test]
    fn test_sort_case_insensitive() {
        let mut entries = vec![
            make_entry("B", "file", 0, 0),
            make_entry("a", "file", 0, 0),
            make_entry("c", "file", 0, 0),
        ];
        sort_entries(&mut entries, "name", "asc");
        assert_eq!(entries[0].name, "a");
        assert_eq!(entries[1].name, "B");
        assert_eq!(entries[2].name, "c");
    }

    #[test]
    fn test_sort_invalid_field_falls_back_to_name() {
        let mut entries = vec![make_entry("b", "file", 0, 0), make_entry("a", "file", 0, 0)];
        sort_entries(&mut entries, "invalid_field", "asc");
        assert_eq!(entries[0].name, "a");
        assert_eq!(entries[1].name, "b");
    }
}
