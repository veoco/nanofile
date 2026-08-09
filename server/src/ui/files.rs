/// Web UI file browser handlers.
use askama::Template;
use axum::{
    extract::{Path, Query, State},
    http::{HeaderMap, header},
    response::{Html, IntoResponse},
};
use serde::Deserialize;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use crate::AppState;
use crate::fs::core::download::Downloader;
use crate::i18n::I18n;
use base::error::AppError;
use infra::common::DirEntry;
use infra::common::util::{basename, parent_path_from};

use super::auth_extractor::WebUser;

// ─── Templates ───────────────────────────────────────────────────────────────

#[derive(Template)]
#[template(path = "files/browser.html")]
pub struct FileBrowserTemplate {
    pub urls: &'static crate::static_assets::TemplateUrls,
    pub t: &'static I18n,
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
    /// Distinct tags used in the current folder, for the sort-bar filter.
    pub folder_tags: Vec<TagChip>,
    /// The currently active tag filter (tag name), if any.
    pub current_tag: Option<String>,
}

#[derive(Template)]
#[template(path = "files/browser_core.html")]
pub struct FileBrowserCoreTemplate {
    pub urls: &'static crate::static_assets::TemplateUrls,
    pub t: &'static I18n,
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
    /// Distinct tags used in the current folder, for the sort-bar filter.
    pub folder_tags: Vec<TagChip>,
    /// The currently active tag filter (tag name), if any.
    pub current_tag: Option<String>,
}

#[derive(Template)]
#[template(path = "files/preview_text.html")]
pub struct PreviewTextTemplate {
    pub urls: &'static crate::static_assets::TemplateUrls,
    pub t: &'static I18n,
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
    pub t: &'static I18n,
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
    /// Thumbnail URL for image/audio/video files at list-view scale (48px), None otherwise.
    pub image_thumbnail_url: Option<String>,
    /// Thumbnail URL for image/audio/video files at grid-view scale (256px), None otherwise.
    pub image_thumbnail_url_large: Option<String>,
    /// Whether this file is a video (used for gallery/right-panel rendering).
    pub is_video: bool,
    /// Whether this file is an audio file (inline playback + cover thumbnails).
    pub is_audio: bool,
    /// Email of the user who last modified this entry.
    pub modifier_email: String,
    /// Tags attached to this entry (name + color), for rendering tag chips.
    pub tags: Vec<TagChip>,
    /// Metadata record id (hex-encoded path) used by the tag editor APIs.
    pub record_id: String,
}

/// A display tag attached to a file entry.
#[derive(Clone)]
pub struct TagChip {
    pub name: String,
    pub color: String,
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
        .map(|e| crate::thumbnail_util::is_video_ext(&e.to_ascii_lowercase()))
        .unwrap_or(false)
}

/// Returns true if the file extension indicates an audio file.
/// Used to enable inline playback and cover-art thumbnails.
pub fn is_audio_file(name: &str) -> bool {
    std::path::Path::new(name)
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| crate::thumbnail_util::is_audio_ext(&e.to_ascii_lowercase()))
        .unwrap_or(false)
}

/// Format a unix timestamp into a month label like "June 2026".
pub fn format_month_label(t: &I18n, timestamp: i64) -> String {
    chrono::DateTime::from_timestamp(timestamp, 0)
        .map(|dt| {
            if t.lang.starts_with("zh") {
                dt.format("%Y年%m月").to_string()
            } else {
                dt.format("%B %Y").to_string()
            }
        })
        .unwrap_or_else(|| t.tr("common.unknown").to_string())
}

/// Group already-sorted (by mtime descending) entries by calendar month.
/// Returns groups in descending month order (newest first).
pub fn group_entries_by_month(t: &I18n, entries: Vec<FileEntry>) -> Vec<GalleryMonthGroup> {
    let mut groups: Vec<GalleryMonthGroup> = Vec::new();
    for entry in entries {
        let label = format_month_label(t, entry.mtime);
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

/// Build the full path (leading `/`) for a directory entry, matching the
/// handler's prior `FileEntry` construction logic.
fn entry_full_path(path: &str, name: &str) -> String {
    if path == "/" {
        format!("/{name}")
    } else {
        format!("{}/{}", path.trim_end_matches('/'), name)
    }
}

/// Sort dirents exactly like `sort_entries` sorts `FileEntry`: directories
/// always first, then by name/mtime/size with the requested order.
fn sort_dirents(dirents: &mut [DirEntry], sort: &str, sort_order: &str) {
    dirents.sort_by(|a, b| {
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

/// A media dirent is a file that can show a thumbnail (image/audio/video).
fn is_media_dirent(e: &DirEntry) -> bool {
    e.entry_type == "file"
        && (is_video_file(&e.name) || is_audio_file(&e.name) || is_thumbnail_image(&e.name))
}

/// Build the display `FileEntry` for a single dirent (called only on the
/// current page slice after sorting/pagination, so cost scales with page size).
fn build_file_entry(
    t: &I18n,
    repo_id: &str,
    path: &str,
    e: &DirEntry,
    starred_set: &HashSet<String>,
    tags_by_path: &HashMap<String, Vec<TagChip>>,
) -> FileEntry {
    let relative_path = if path == "/" {
        e.name.clone()
    } else {
        format!("{}/{}", path.trim_start_matches('/'), e.name)
    };
    let full_path = entry_full_path(path, &e.name);
    let is_previewable = is_previewable_file(&e.name);
    let ext = if e.entry_type == "file" {
        file_extension(&e.name)
    } else {
        None
    };
    let is_image_file = e.entry_type == "file" && is_thumbnail_image(&e.name);
    let entry_is_video = e.entry_type == "file" && is_video_file(&e.name);
    let entry_is_audio = e.entry_type == "file" && is_audio_file(&e.name);
    // Images get in-process thumbnails; audio/video via ffmpeg (frame or
    // embedded cover art). Audio without cover art falls back to an
    // extension badge on the client (thumbnail endpoint returns 404).
    let needs_thumb = is_image_file || entry_is_video || entry_is_audio;
    let thumb_url = if needs_thumb {
        Some(format!(
            "/api2/repos/{}/thumbnail/?p={}&size=48",
            repo_id,
            urlencode_path(&full_path)
        ))
    } else {
        None
    };
    let thumb_url_large = if needs_thumb {
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
        entry_type: e.entry_type.clone(),
        size: e.size,
        size_display: format_size(e.size),
        mtime: e.mtime,
        mtime_display: format_mtime(t, e.mtime),
        icon_color: file_icon_color(&e.name),
        relative_path,
        is_previewable,
        starred: starred_set.contains(&full_path),
        extension: ext,
        image_thumbnail_url: thumb_url,
        image_thumbnail_url_large: thumb_url_large,
        is_video: entry_is_video,
        is_audio: entry_is_audio,
        modifier_email: e.modifier.clone(),
        tags: tags_by_path.get(&full_path).cloned().unwrap_or_default(),
        record_id: crate::service::fs::metadata::MetadataService::record_id_from_path(&full_path),
    }
}

/// Returns true if the file extension is one that the thumbnail service supports
/// for generating image thumbnails (in-process image formats, plus HEIC/HEIF/AVIF
/// decoded via ffmpeg).
fn is_thumbnail_image(name: &str) -> bool {
    std::path::Path::new(name)
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| crate::thumbnail_util::is_thumbnail_image_ext(&e.to_lowercase()))
        .unwrap_or(false)
}

/// Returns true if the file is a still image the browser can render in the
/// full-page preview. Includes SVG (browser-native, no thumbnail) plus the
/// ffmpeg-only formats (HEIC/HEIF/AVIF). Case-insensitive.
fn is_preview_image_file(name: &str) -> bool {
    std::path::Path::new(name)
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| {
            matches!(
                e.to_lowercase().as_str(),
                "png"
                    | "jpg"
                    | "jpeg"
                    | "gif"
                    | "webp"
                    | "bmp"
                    | "svg"
                    | "tiff"
                    | "tif"
                    | "heic"
                    | "heif"
                    | "avif"
            )
        })
        .unwrap_or(false)
}

pub fn is_previewable_file(name: &str) -> bool {
    let name = &name.to_ascii_lowercase();
    // Images
    if is_preview_image_file(name) {
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
    if is_preview_image_file(name) {
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

pub use super::format_size;

pub fn format_mtime(t: &I18n, timestamp: i64) -> String {
    chrono::DateTime::from_timestamp(timestamp, 0)
        .map(|dt| {
            if t.lang.starts_with("zh") {
                dt.format("%Y年%m月%d日 %H:%M").to_string()
            } else {
                dt.format("%Y-%m-%d %H:%M").to_string()
            }
        })
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
    /// Filter the current folder to entries carrying this tag name.
    pub tag: Option<String>,
}

// ─── Helpers ─────────────────────────────────────────────────────────────────

async fn verify_repo_access(
    member_repo: &dyn crate::repository::member::MemberRepository,
    user_id: i32,
    repo_id: &str,
) -> Result<(), AppError> {
    crate::domain::permission::check_repo_read_permission(member_repo, repo_id, user_id).await?;
    Ok(())
}

// ─── Handlers ────────────────────────────────────────────────────────────────

/// GET /library/{id}/{name}/ — repo file browser (root).
pub async fn file_browser_root(
    user: WebUser,
    State(state): State<Arc<AppState>>,
    Path(repo_id): Path<String>,
    headers: HeaderMap,
    Query(query): Query<FileBrowserQuery>,
) -> Result<impl IntoResponse, AppError> {
    file_browser_inner(user, state, repo_id, "/".to_string(), headers, query).await
}

/// GET /library/{id}/{name}/{*path} — repo file browser (any path).
pub async fn file_browser(
    user: WebUser,
    State(state): State<Arc<AppState>>,
    Path((repo_id, path)): Path<(String, String)>,
    headers: HeaderMap,
    Query(query): Query<FileBrowserQuery>,
) -> Result<impl IntoResponse, AppError> {
    let path = base::sanitize::safe_normalize_path(&path)
        .map_err(|e| AppError::BadRequest(format!("Invalid path: {e}")))?;
    file_browser_inner(user, state, repo_id, path, headers, query).await
}

async fn file_browser_inner(
    user: WebUser,
    state: Arc<AppState>,
    repo_id: String,
    path: String,
    headers: HeaderMap,
    query: FileBrowserQuery,
) -> Result<impl IntoResponse, AppError> {
    let t = I18n::get(user.language.as_deref());
    let repos = &state.repos;
    verify_repo_access(state.repos.member.as_ref(), user.user_id, &repo_id).await?;

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
        crate::service::fs::dir::list_dir_from_fs_tree(repos, &repo_id, &path).await;

    let entries_data = match entries_result {
        Ok(data) => data,
        Err(_e) if path == "/" => {
            // Root path listing failed → render empty directory (empty repo).
            (String::new(), vec![])
        }
        Err(AppError::NotFound(_)) => {
            // Path doesn't resolve as a directory — likely points to a file.
            // Fall through to file serving.
            return serve_file(user, state, repo_id, path, headers, query)
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

    // Batch-load tags for the current folder's entries so each row can render
    // its tag chips and the sort-bar filter knows which tags exist here.
    let folder_paths: Vec<String> = entries_data
        .1
        .iter()
        .map(|e| {
            if path == "/" {
                format!("/{}", e.name)
            } else {
                format!("{}/{}", path.trim_end_matches('/'), e.name)
            }
        })
        .collect();
    let tag_details = state
        .repos
        .file_tag
        .find_tag_details_by_paths(&repo_id, &folder_paths)
        .await?;
    let mut tags_by_path: HashMap<String, Vec<TagChip>> = HashMap::new();
    let mut folder_tags: Vec<TagChip> = Vec::new();
    for d in tag_details {
        let chip = TagChip {
            name: d.tag_name.clone(),
            color: d.tag_color.clone(),
        };
        tags_by_path
            .entry(d.file_path.clone())
            .or_default()
            .push(chip);
        if !folder_tags.iter().any(|t| t.name == d.tag_name) {
            folder_tags.push(TagChip {
                name: d.tag_name,
                color: d.tag_color,
            });
        }
    }

    // Sort/filter/paginate on the lightweight dirent level first, then build
    // `FileEntry` only for the current page slice. This keeps the cost
    // proportional to the page size instead of the whole folder.
    let mut dirents = entries_data.1;

    // Apply the tag filter (current folder only, non-recursive).
    if let Some(tag) = query.tag.as_deref().filter(|t| !t.is_empty()) {
        dirents.retain(|e| {
            let full_path = entry_full_path(&path, &e.name);
            tags_by_path
                .get(&full_path)
                .map(|chips| chips.iter().any(|c| c.name == tag))
                .unwrap_or(false)
        });
    }

    let total = dirents.len() as i64;
    let per_page = query.per_page.unwrap_or(200).min(500) as usize;
    let page = query.page.unwrap_or(1).max(1) as usize;

    // Determine view mode before building so we only pay for the active view.
    let render_view = match query.view.as_deref() {
        Some("list") => "list",
        Some("grid") => "grid",
        Some("gallery") => "gallery",
        _ => "all",
    };

    // Sort: directories first, then by configurable field and order
    let sort_field = query.sort.as_deref().unwrap_or("name");
    let sort_order = query.sort_order.as_deref().unwrap_or("asc");

    // list/grid view: sort dirents, paginate, then build FileEntry per page row.
    let entries: Vec<FileEntry>;
    let has_more: bool;
    if render_view == "list" || render_view == "grid" || render_view == "all" {
        sort_dirents(&mut dirents, sort_field, sort_order);
        let offset = (page - 1) * per_page;
        if offset < dirents.len() {
            let end = (offset + per_page).min(dirents.len());
            has_more = end < dirents.len();
            entries = dirents[offset..end]
                .iter()
                .map(|d| build_file_entry(t, &repo_id, &path, d, &starred_set, &tags_by_path))
                .collect();
        } else {
            has_more = false;
            entries = Vec::new();
        }
    } else {
        has_more = false;
        entries = Vec::new();
    }

    // Gallery view: media dirents sorted by mtime desc, independently paginated,
    // then FileEntry built only for the page. Gallery keeps reverse-chronological
    // order regardless of the configured sort used by list/grid views.
    let gallery_groups: Vec<GalleryMonthGroup>;
    let gallery_total: i64;
    if render_view == "gallery" || render_view == "all" {
        let mut media: Vec<&DirEntry> = dirents.iter().filter(|d| is_media_dirent(d)).collect();
        media.sort_by_key(|d| std::cmp::Reverse(d.mtime)); // mtime descending
        gallery_total = media.len() as i64;
        let offset = (page - 1) * per_page;
        let paginated: Vec<FileEntry> = if offset < media.len() {
            let end = (offset + per_page).min(media.len());
            media[offset..end]
                .iter()
                .map(|d| build_file_entry(t, &repo_id, &path, d, &starred_set, &tags_by_path))
                .collect()
        } else {
            Vec::new()
        };
        gallery_groups = group_entries_by_month(t, paginated);
    } else {
        gallery_groups = Vec::new();
        gallery_total = 0;
    }

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
            t,
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
            folder_tags: folder_tags.clone(),
            current_tag: query.tag.clone().filter(|t| !t.is_empty()),
        };
        let html = tpl
            .render()
            .map_err(|e| AppError::internal(e.to_string()))?;
        Ok(Html(html).into_response())
    } else {
        let ctx = crate::ui::ctx::build_page_ctx(&state, &user).await?;
        let current_repo_id = Some(repo_id.clone());
        let tpl = FileBrowserTemplate {
            urls: ctx.urls,
            t: ctx.t,
            user_email: ctx.user_email,
            is_admin: ctx.is_admin,
            csrf_token: ctx.csrf_token,
            repo_id,
            repo_name: repo_record.name,
            current_path: path,
            breadcrumbs,
            entries,
            total: effective_total,
            has_more: effective_has_more,
            page: page as u32,
            render_view,
            active_page: "repos",
            left_panel_repos: ctx.left_panel_repos,
            current_repo_id,
            max_upload_size_mb: state.config.server.max_upload_size_mb,
            sort_field: sort_field.to_string(),
            sort_order: sort_order.to_string(),
            gallery_groups,
            folder_tags,
            current_tag: query.tag.filter(|t| !t.is_empty()),
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
    headers: HeaderMap,
    query: FileBrowserQuery,
) -> Result<impl IntoResponse, AppError> {
    let path = base::sanitize::safe_normalize_path(&path)
        .map_err(|e| AppError::BadRequest(format!("Invalid path: {e}")))?;
    let file_name = path.rsplit('/').next().unwrap_or("file").to_string();

    // ?dl=1 → force download (streamed so large files aren't buffered).
    if query.dl.as_deref() == Some("1") {
        let (file_data, block_ids) = Downloader::resolve_blocks(&state.repos, &repo_id, &path)
            .await
            .map_err(|e| AppError::Internal(format!("download failed: {e}")))?;
        let total = file_data.size.max(0) as u64;
        let disposition = format!("attachment; filename=\"{}\"", file_name);
        let range_header = headers.get(header::RANGE).and_then(|v| v.to_str().ok());
        return Ok(crate::fs::core::download::file_download_response(
            crate::fs::core::download::FileDownloadParams {
                block_ids,
                block_store: state.block_store.clone(),
                enc_key: None,
                total_size: total,
                content_type: mime_guess(&file_name),
                content_disposition: Some(disposition),
                range_header: range_header.map(|s| s.to_string()),
            },
        ));
    }

    // Audio/video — stream inline with Range support so the HTML5 player can seek.
    if is_video_file(&file_name) || is_audio_file(&file_name) {
        let (file_data, block_ids) = Downloader::resolve_blocks(&state.repos, &repo_id, &path)
            .await
            .map_err(|e| AppError::Internal(format!("download failed: {e}")))?;
        let total = file_data.size.max(0) as u64;
        let range_header = headers.get(header::RANGE).and_then(|v| v.to_str().ok());
        return Ok(crate::fs::core::download::file_download_response(
            crate::fs::core::download::FileDownloadParams {
                block_ids,
                block_store: state.block_store.clone(),
                enc_key: None,
                total_size: total,
                content_type: mime_guess(&file_name),
                content_disposition: None,
                range_header: range_header.map(|s| s.to_string()),
            },
        ));
    }

    // Image preview
    let is_image = is_preview_image_file(&file_name);

    // Text/code preview
    let is_text = is_previewable_file(&file_name);

    if is_image {
        let size_display = get_file_size(&state.db, &state.repos, &repo_id, &path)
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
            t: I18n::get(user.language.as_deref()),
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
        // Cap the preview read so huge text files don't blow up memory.
        let data = Downloader::download_file_limited(
            &state.repos,
            &repo_id,
            &path,
            &state.block_store,
            None,
            4 * 1024 * 1024,
        )
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

        let size_display = get_file_size(&state.db, &state.repos, &repo_id, &path)
            .await
            .map(format_size)
            .unwrap_or_else(|_| "?".to_string());

        let left_panel_repos =
            crate::service::repo::service::load_left_panel_repos(&state.repos, user.user_id)
                .await?;
        let tpl = PreviewTextTemplate {
            urls: crate::static_assets::template_urls(),
            t: I18n::get(user.language.as_deref()),
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

    // Binary files — serve raw bytes inline (streamed).
    let (file_data, block_ids) = Downloader::resolve_blocks(&state.repos, &repo_id, &path)
        .await
        .map_err(|e| AppError::Internal(format!("download failed: {e}")))?;
    let total = file_data.size.max(0) as u64;
    let range_header = headers.get(header::RANGE).and_then(|v| v.to_str().ok());
    Ok(crate::fs::core::download::file_download_response(
        crate::fs::core::download::FileDownloadParams {
            block_ids,
            block_store: state.block_store.clone(),
            enc_key: None,
            total_size: total,
            content_type: mime_guess(&file_name),
            content_disposition: None,
            range_header: range_header.map(|s| s.to_string()),
        },
    ))
}

/// Resolve a file's size from the FS tree without downloading its content.
async fn get_file_size(
    db: &sea_orm::DatabaseConnection,
    repos: &crate::repository::Repositories,
    repo_id: &str,
    path: &str,
) -> Result<i64, AppError> {
    let head_root_id = infra::common::util::get_head_root_id(db, repo_id).await?;
    let parent_path = parent_path_from(path);
    let file_name = basename(path);

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

pub(crate) fn mime_guess(filename: &str) -> &'static str {
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
    } else if filename.ends_with(".mp4") {
        "video/mp4"
    } else if filename.ends_with(".webm") {
        "video/webm"
    } else if filename.ends_with(".mov") {
        "video/quicktime"
    } else if filename.ends_with(".3gp") {
        "video/3gpp"
    } else if filename.ends_with(".mkv") {
        "video/x-matroska"
    } else if filename.ends_with(".avi") {
        "video/x-msvideo"
    } else if filename.ends_with(".wmv") {
        "video/x-ms-wmv"
    } else if filename.ends_with(".flv") {
        "video/x-flv"
    } else if filename.ends_with(".mp3") {
        "audio/mpeg"
    } else if filename.ends_with(".flac") {
        "audio/flac"
    } else if filename.ends_with(".wav") {
        "audio/wav"
    } else if filename.ends_with(".ogg") || filename.ends_with(".opus") {
        "audio/ogg"
    } else if filename.ends_with(".m4a") {
        "audio/mp4"
    } else if filename.ends_with(".aac") {
        "audio/aac"
    } else if filename.ends_with(".wma") {
        "audio/x-ms-wma"
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
            is_audio: false,
            modifier_email: String::new(),
            tags: Vec::new(),
            record_id: String::new(),
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

    fn make_dirent(name: &str, entry_type: &str, size: i64, mtime: i64) -> DirEntry {
        DirEntry {
            id: String::new(),
            entry_type: entry_type.to_string(),
            name: name.to_string(),
            size,
            mtime,
            permission: "rw".to_string(),
            modifier: String::new(),
            parent_dir: None,
            modifier_name: None,
            modifier_contact_email: None,
        }
    }

    #[test]
    fn test_sort_dirents_name_asc_dirs_first() {
        let mut dirents = vec![
            make_dirent("b", "file", 0, 3),
            make_dirent("a", "dir", 0, 2),
            make_dirent("c", "file", 0, 1),
            make_dirent("d", "dir", 0, 4),
        ];
        sort_dirents(&mut dirents, "name", "asc");
        assert_eq!(dirents[0].name, "a"); // dir first
        assert_eq!(dirents[1].name, "d"); // dir second
        assert_eq!(dirents[2].name, "b"); // file
        assert_eq!(dirents[3].name, "c");
    }

    #[test]
    fn test_sort_dirents_name_desc() {
        let mut dirents = vec![
            make_dirent("b", "file", 0, 0),
            make_dirent("a", "file", 0, 0),
            make_dirent("c", "dir", 0, 0),
        ];
        sort_dirents(&mut dirents, "name", "desc");
        assert_eq!(dirents[0].name, "c"); // dir first
        assert_eq!(dirents[1].name, "b"); // files: desc order
        assert_eq!(dirents[2].name, "a");
    }

    #[test]
    fn test_sort_dirents_size_and_mtime() {
        let mut by_size = vec![
            make_dirent("big", "file", 1000, 0),
            make_dirent("small", "file", 10, 0),
            make_dirent("dir1", "dir", 999, 0),
        ];
        sort_dirents(&mut by_size, "size", "asc");
        assert_eq!(by_size[0].name, "dir1");
        assert_eq!(by_size[1].name, "small");
        assert_eq!(by_size[2].name, "big");

        let mut by_mtime = vec![
            make_dirent("old", "file", 0, 10),
            make_dirent("new", "file", 0, 100),
            make_dirent("dir1", "dir", 0, 50),
        ];
        sort_dirents(&mut by_mtime, "mtime", "desc");
        assert_eq!(by_mtime[0].name, "dir1");
        assert_eq!(by_mtime[1].name, "new");
        assert_eq!(by_mtime[2].name, "old");
    }

    #[test]
    fn test_sort_dirents_case_insensitive() {
        let mut dirents = vec![
            make_dirent("B", "file", 0, 0),
            make_dirent("a", "file", 0, 0),
            make_dirent("c", "file", 0, 0),
        ];
        sort_dirents(&mut dirents, "name", "asc");
        assert_eq!(dirents[0].name, "a");
        assert_eq!(dirents[1].name, "B");
        assert_eq!(dirents[2].name, "c");
    }
}
