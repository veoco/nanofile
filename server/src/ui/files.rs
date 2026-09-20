/// Web UI file browser handlers.
use askama::Template;
use axum::{
    extract::{Path, Query, State},
    response::{Html, IntoResponse, Redirect},
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

/// Largest page number the file browser accepts. `page * per_page` is used as
/// a slice offset, so the value must stay far away from `usize` overflow on
/// 32-bit targets while still being unreachable in practice.
const MAX_BROWSER_PAGE: u32 = 10_000;

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
    /// Whether the *currently rendered view* has content (drives the folder-level
    /// empty state). For gallery-only renders `entries` is intentionally empty
    /// (gallery is built from `gallery_groups`), so the empty state must be
    /// view-aware instead of gating on `entries.is_empty()`.
    pub is_empty: bool,
    pub total: i64,
    pub has_more: bool,
    /// Gallery paginates the media subset independently (mtime-desc), so its
    /// total/has_more differ from the list/grid all-file counts.
    pub gallery_total: i64,
    pub gallery_has_more: bool,
    /// Photo/video split for the sort-bar summary shown in gallery mode.
    pub gallery_photo_total: i64,
    pub gallery_video_total: i64,
    pub page: u32,
    /// "all" = render all three views (full page), "list" = only list,
    /// "grid" = only grid, "gallery" = only gallery
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
    /// Whether the *currently rendered view* has content (drives the folder-level
    /// empty state). For gallery-only renders `entries` is intentionally empty
    /// (gallery is built from `gallery_groups`), so the empty state must be
    /// view-aware instead of gating on `entries.is_empty()`.
    pub is_empty: bool,
    pub total: i64,
    pub has_more: bool,
    /// Gallery paginates the media subset independently (mtime-desc), so its
    /// total/has_more differ from the list/grid all-file counts.
    pub gallery_total: i64,
    pub gallery_has_more: bool,
    /// Photo/video split for the sort-bar summary shown in gallery mode.
    pub gallery_photo_total: i64,
    pub gallery_video_total: i64,
    pub page: u32,
    /// "all" = render all three views (full page), "list" = only list,
    /// "grid" = only grid, "gallery" = only gallery
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
    /// Unified `/repos/...` URL the preview embeds (or links to).
    pub content_url: String,
    /// The same URL with `?dl=1`, for the download action.
    pub download_url: String,
    pub parent_path: String,
    /// Crumb trail ending at the file itself, for the shared breadcrumb
    /// include (its last item renders as the non-link current page).
    pub breadcrumbs: Vec<BreadcrumbItem>,
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
    /// Unified `/repos/...` URL the preview embeds (or links to).
    pub content_url: String,
    /// The same URL with `?dl=1`, for the download action.
    pub download_url: String,
    pub parent_path: String,
    /// Crumb trail ending at the file itself, for the shared breadcrumb
    /// include (its last item renders as the non-link current page).
    pub breadcrumbs: Vec<BreadcrumbItem>,
    pub size_display: String,
    pub active_page: &'static str,
    pub left_panel_repos: Vec<crate::service::repo::service::LeftPanelRepo>,
    pub current_repo_id: Option<String>,
}

/// Video/audio preview page. Embeds an HTML5 `<video>`/`<audio>` whose `src`
/// points at the unified `/repos/...` content endpoint (Range-capable), so
/// `/libraries/...` itself never serves media bytes.
#[derive(Template)]
#[template(path = "files/preview_media.html")]
pub struct PreviewMediaTemplate {
    pub urls: &'static crate::static_assets::TemplateUrls,
    pub t: &'static I18n,
    pub user_email: String,
    pub is_admin: bool,
    pub repo_name: String,
    pub file_name: String,
    pub repo_id: String,
    /// Unified `/repos/...` URL the preview embeds (or links to).
    pub content_url: String,
    /// The same URL with `?dl=1`, for the download action.
    pub download_url: String,
    pub parent_path: String,
    /// Crumb trail ending at the file itself, for the shared breadcrumb
    /// include (its last item renders as the non-link current page).
    pub breadcrumbs: Vec<BreadcrumbItem>,
    pub size_display: String,
    pub active_page: &'static str,
    pub left_panel_repos: Vec<crate::service::repo::service::LeftPanelRepo>,
    pub current_repo_id: Option<String>,
    pub is_video: bool,
}

// ─── Data types ──────────────────────────────────────────────────────────────

#[derive(Clone)]
pub struct FileEntry {
    pub name: String,
    pub entry_type: String, // "file" or "dir"
    pub size: i64,
    pub size_display: String,
    pub mtime: i64,
    pub icon_color: &'static str,
    /// Relative path for use in URL construction, e.g. "Documents/file.txt"
    pub relative_path: String,
    /// Whether this file can be previewed inline (text/code/image).
    pub is_previewable: bool,
    /// Whether this file/directory is starred by the current user.
    pub starred: bool,
    /// File extension in uppercase (e.g. "PDF", "PNG"), None for directories.
    pub extension: Option<String>,
    /// `name` without its extension, for the two-tone "name + .ext" display.
    pub name_base: String,
    /// Lowercase dotted extension (e.g. ".pdf"), empty when there is none.
    pub ext_display: String,
    /// Which thumbnail treatment the grid/gallery give this entry: "dir",
    /// "image", "video" or "doc". See `build_file_entry`.
    pub tile_kind: &'static str,
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

/// Thumbnail sizes the file browser asks for — the layout half of the story.
///
/// The values and the reasoning behind them live in `thumbnail_util`
/// (`THUMBNAIL_SIZE_SMALL` / `THUMBNAIL_SIZE_LARGE`), next to
/// `RETAINED_THUMBNAIL_SIZES`: the cache purge has to agree with this file
/// about which sizes are still live.
///
/// Whether a dirent belongs in the gallery contact sheet.
///
/// Audio is deliberately excluded: a photo/video sheet has nothing to show for
/// a sound file, and it stays reachable in the list and grid views. (The
/// thumbnail endpoint also 404s for audio without cover art, so an included
/// `.mp3` would only ever be a grey square with "MP3" on it.)
fn is_gallery_media_dirent(e: &DirEntry) -> bool {
    e.entry_type == "file" && (is_video_file(&e.name) || is_thumbnail_image(&e.name))
}

/// Build the display `FileEntry` for a single dirent (called only on the
/// current page slice after sorting/pagination, so cost scales with page size).
fn build_file_entry(
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
            "/api2/repos/{}/thumbnail/?p={}&size={}",
            repo_id,
            urlencode_path(&full_path),
            crate::thumbnail_util::THUMBNAIL_SIZE_SMALL
        ))
    } else {
        None
    };
    let thumb_url_large = if needs_thumb {
        Some(format!(
            "/api2/repos/{}/thumbnail/?p={}&size={}",
            repo_id,
            urlencode_path(&full_path),
            crate::thumbnail_util::THUMBNAIL_SIZE_LARGE
        ))
    } else {
        None
    };
    let (name_base, ext_display) = split_display_name(&e.name, &e.entry_type);
    // How the grid/gallery should draw this entry, decided once here so the
    // templates stay free of nested `if let` chains:
    //   "dir"   folder glyph
    //   "image" real thumbnail
    //   "video" thumbnail if we have one, plus a play badge
    //   "doc"   hairline outline box with a glyph + extension
    let tile_kind = if e.entry_type == "dir" {
        "dir"
    } else if entry_is_video {
        "video"
    } else if needs_thumb {
        "image"
    } else {
        "doc"
    };
    FileEntry {
        name: e.name.clone(),
        entry_type: e.entry_type.clone(),
        size: e.size,
        size_display: format_size(e.size),
        mtime: e.mtime,
        icon_color: file_icon_color(&e.name),
        relative_path,
        is_previewable,
        starred: starred_set.contains(&full_path),
        extension: ext,
        name_base,
        ext_display,
        tile_kind,
        image_thumbnail_url: thumb_url,
        image_thumbnail_url_large: thumb_url_large,
        is_video: entry_is_video,
        is_audio: entry_is_audio,
        modifier_email: e.modifier.clone(),
        tags: tags_by_path.get(&full_path).cloned().unwrap_or_default(),
        record_id: crate::service::fs::metadata::MetadataService::record_id_from_path(&full_path),
    }
}

/// Group tag details into a per-path chip map plus the distinct tags used in
/// the folder (for the sort-bar filter).
fn build_tags_map(
    details: Vec<crate::repository::file_tag::TagOnPath>,
) -> (HashMap<String, Vec<TagChip>>, Vec<TagChip>) {
    let mut tags_by_path: HashMap<String, Vec<TagChip>> = HashMap::new();
    let mut folder_tags: Vec<TagChip> = Vec::new();
    for d in details {
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
    (tags_by_path, folder_tags)
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

/// Colour class for the file-type badge.
///
/// Graphite is a zero-chroma UI: chromatic colour is reserved for semantic
/// state (ok / warn / err), so file types no longer carry a per-kind hue. The
/// function is kept because the badge is rendered from Rust-side class strings
/// (which `build.rs` scans for Tailwind) in several templates.
fn file_icon_color(_name: &str) -> &'static str {
    "text-ink-2"
}

/// Split a display name into its stem and a lowercase dotted suffix.
///
/// Mirrors `file_extension`'s rules — no suffix, or a suffix containing `/`, is
/// not an extension — but additionally keeps dotfiles whole (`.bashrc` is a
/// name, not a stem plus `.bashrc`).
fn split_display_name(name: &str, entry_type: &str) -> (String, String) {
    if entry_type != "file" {
        return (name.to_string(), String::new());
    }
    match name.rsplit_once('.') {
        Some((stem, ext)) if !stem.is_empty() && !ext.is_empty() && !ext.contains('/') => {
            (stem.to_string(), format!(".{}", ext.to_lowercase()))
        }
        _ => (name.to_string(), String::new()),
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

/// Relative time matching the Android client's `translateCommitTime`:
/// "Just now" → N seconds → N minutes → N hours → N days → absolute date
/// (>= 14 days). A count of 1 gets its own key, so nothing reads "1 days ago".
pub fn format_relative_time(t: &I18n, now: i64, timestamp: i64) -> String {
    let diff = now - timestamp;
    if diff <= 0 {
        return t.tr("activity.just_now").to_string();
    }
    let seconds = diff;
    let minutes = seconds / 60;
    let hours = seconds / 3600;
    let days = seconds / 86400;

    if days >= 14 {
        return chrono::DateTime::from_timestamp(timestamp, 0)
            .map(|dt| {
                if t.lang.starts_with("zh") {
                    dt.format("%Y年%m月%d日").to_string()
                } else {
                    dt.format("%Y-%m-%d").to_string()
                }
            })
            .unwrap_or_else(|| t.tr("activity.just_now").to_string());
    }
    if days > 0 {
        return amount(t, days, "activity.day_ago", "activity.days_ago");
    }
    if hours > 0 {
        return amount(t, hours, "activity.hour_ago", "activity.hours_ago");
    }
    if minutes > 0 {
        return amount(t, minutes, "activity.minute_ago", "activity.minutes_ago");
    }
    amount(t, seconds, "activity.second_ago", "activity.seconds_ago")
}

/// "1 day ago", not "1 days ago" — the count is 1 far more often than any other
/// single value on a list of things you just touched, so it is worth its own key.
fn amount(t: &I18n, n: i64, one: &str, many: &str) -> String {
    if n == 1 {
        t.tr(one).to_string()
    } else {
        t.trf(many, &[("n", n.to_string())])
    }
}

/// UTC calendar-day key (`YYYY-MM-DD`) used to group activity rows by day.
pub fn day_key(timestamp: i64) -> String {
    chrono::DateTime::from_timestamp(timestamp, 0)
        .map(|dt| dt.format("%Y-%m-%d").to_string())
        .unwrap_or_default()
}

#[derive(Clone)]
pub struct BreadcrumbItem {
    pub label: String,
    pub path: String,
}

/// Crumb trail for a repo path, each item carrying the path up to and including
/// itself (relative, no leading `/`, so it drops straight into a URL).
///
/// Used both for a directory (the browser's trail) and for a file (the preview
/// page's trail, where the last crumb is the file and renders as the current
/// page rather than a link).
pub(crate) fn breadcrumbs_for(path: &str) -> Vec<BreadcrumbItem> {
    let mut breadcrumbs = Vec::new();
    if path == "/" {
        return breadcrumbs;
    }
    let mut accum = String::new();
    for seg in path
        .trim_start_matches('/')
        .split('/')
        .filter(|s| !s.is_empty())
    {
        if !accum.is_empty() {
            accum.push('/');
        }
        accum.push_str(seg);
        breadcrumbs.push(BreadcrumbItem {
            label: seg.to_string(),
            path: accum.clone(),
        });
    }
    breadcrumbs
}

// ─── Request types ───────────────────────────────────────────────────────────

#[derive(Clone, Deserialize)]
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
    Query(query): Query<FileBrowserQuery>,
) -> Result<impl IntoResponse, AppError> {
    file_browser_inner(user, state, repo_id, "/".to_string(), query).await
}

/// GET /libraries/{id}/files/{*path} — repo file browser (any path).
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

/// `GET /library/{id}/{name}/` — seahub-compatible spelling of the library root.
///
/// The desktop client's "view on website" hands this URL to the browser after
/// auto-login (`seafile-client/src/ui/repo-tree-view.cpp:578` builds
/// `/library/<repo-id>/<name>/`), and only the repository id is meaningful —
/// seahub carries the name as a decorative second segment. Redirect to this
/// server's own path so the address bar, bookmarks and every link rendered on
/// the page settle on one canonical URL.
pub async fn library_redirect_root(
    Path((repo_id, _name)): Path<(String, String)>,
) -> Result<Redirect, AppError> {
    library_redirect(repo_id, None)
}

/// `GET /library/{id}/{name}/{*path}` — seahub-compatible path inside a library.
pub async fn library_redirect_path(
    Path((repo_id, _name, path)): Path<(String, String, String)>,
) -> Result<Redirect, AppError> {
    library_redirect(repo_id, Some(path))
}

/// Build the `/libraries/{id}/files/…` redirect, refusing anything that could
/// not have come from a browser navigation.
///
/// The repository id and the path are both percent-decoded before they reach
/// this function, so a `%0d%0a` must not be echoed into `Location`: the id is
/// restricted to the alphabet repository ids actually use, and the path is
/// normalized (rejecting `..`, NUL and friends) and re-encoded per segment.
fn library_redirect(repo_id: String, path: Option<String>) -> Result<Redirect, AppError> {
    if repo_id.is_empty()
        || !repo_id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-')
    {
        return Err(AppError::BadRequest("invalid repository id".into()));
    }
    let target = match path {
        None => format!("/libraries/{repo_id}/files/"),
        Some(path) => {
            let normalized = base::sanitize::safe_normalize_path(&path)
                .map_err(|e| AppError::BadRequest(format!("Invalid path: {e}")))?;
            format!(
                "/libraries/{repo_id}/files/{}",
                encode_repo_path(&normalized)
            )
        }
    };
    Ok(Redirect::to(&target))
}

async fn file_browser_inner(
    user: WebUser,
    state: Arc<AppState>,
    repo_id: String,
    path: String,
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

    // Media / image / text extensions are unambiguously files — serve the
    // preview page directly instead of first attempting (and failing) a
    // directory listing. If the name is actually a directory (misleading
    // extension), the preview errors and we fall back to the listing below.
    let file_name = path.rsplit('/').next().unwrap_or("");
    if is_video_file(file_name) || is_audio_file(file_name) || is_previewable_file(file_name) {
        match serve_file(
            user.clone(),
            state.clone(),
            repo_id.clone(),
            path.clone(),
            query.clone(),
        )
        .await
        {
            Ok(resp) => return Ok(resp.into_response()),
            Err(_) => {
                // Not actually a file — fall through to the directory listing.
            }
        }
    }

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
            return serve_file(user, state, repo_id, path, query)
                .await
                .map(IntoResponse::into_response);
        }
        Err(e) => {
            // Database errors, I/O errors — do NOT mask these as 500 Internal.
            return Err(e);
        }
    };

    // Pagination / view state must be known before deciding how wide the tag
    // and star lookups need to be. Both bounds are clamped: `page` is
    // multiplied by `per_page` below, so an unbounded value is one request away
    // from an overflow (panic in debug, absurd OFFSET in release).
    let per_page = query.per_page.unwrap_or(200).clamp(1, 500) as usize;
    let page = query.page.unwrap_or(1).clamp(1, MAX_BROWSER_PAGE) as usize;

    // Full-page loads always render all three views so view switching is a pure
    // client-side class toggle with no network round-trip. Partial reloads
    // (pagination / post-mutation refresh) render only the requested view.
    let is_partial = query.partial.as_deref() == Some("1");
    let render_view = if is_partial {
        match query.view.as_deref() {
            Some("list") => "list",
            Some("grid") => "grid",
            Some("gallery") => "gallery",
            _ => "all",
        }
    } else {
        "all"
    };

    // Sort: directories first, then by configurable field and order
    let sort_field = query.sort.as_deref().unwrap_or("name");
    let sort_order = query.sort_order.as_deref().unwrap_or("asc");

    let has_tag_filter = query.tag.as_deref().map(|t| !t.is_empty()).unwrap_or(false);

    // Tag lookups must cover the whole folder whenever the sort-bar needs the
    // folder's full tag set (full-page `view=all`) or a tag filter has to scan
    // the whole folder before paginating. Single-view partial loads (pagination
    // via load-more) render only the current page, and the client discards the
    // sort-bar from those responses, so the lookup can be narrowed to the page.
    let needs_full_tags = has_tag_filter || render_view == "all";

    // Sort/filter/paginate on the lightweight dirent level first, then build
    // `FileEntry` only for the current page slice. This keeps the cost
    // proportional to the page size instead of the whole folder.
    let mut dirents = entries_data.1;

    // Batch-load tags so each row can render its tag chips and the sort-bar
    // filter knows which tags exist in the folder.
    let mut tags_by_path: HashMap<String, Vec<TagChip>> = HashMap::new();
    let mut folder_tags: Vec<TagChip> = Vec::new();
    if needs_full_tags {
        let folder_paths: Vec<String> = dirents
            .iter()
            .map(|e| entry_full_path(&path, &e.name))
            .collect();
        let tag_details = state
            .repos
            .file_tag
            .find_tag_details_by_paths(&repo_id, &folder_paths)
            .await?;
        (tags_by_path, folder_tags) = build_tags_map(tag_details);
    }

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

    // list/grid view: sort dirents, then keep only the current page slice.
    let mut list_slice: Vec<&DirEntry> = Vec::new();
    let has_more: bool = if render_view == "list" || render_view == "grid" || render_view == "all" {
        sort_dirents(&mut dirents, sort_field, sort_order);
        let offset = (page - 1) * per_page;
        if offset < dirents.len() {
            let end = (offset + per_page).min(dirents.len());
            let has_more = end < dirents.len();
            list_slice = dirents[offset..end].iter().collect();
            has_more
        } else {
            false
        }
    } else {
        false
    };

    // Gallery view: media dirents sorted by mtime desc, independently
    // paginated. Gallery keeps reverse-chronological order regardless of the
    // configured sort used by list/grid views.
    let mut gallery_slice: Vec<&DirEntry> = Vec::new();
    let mut gallery_photo_total: i64 = 0;
    let mut gallery_video_total: i64 = 0;
    let gallery_total: i64 = if render_view == "gallery" || render_view == "all" {
        let mut media: Vec<&DirEntry> = dirents
            .iter()
            .filter(|d| is_gallery_media_dirent(d))
            .collect();
        media.sort_by_key(|d| std::cmp::Reverse(d.mtime)); // mtime descending
        // Counted over the *whole* folder, not the page, so the sort-bar summary
        // stays put while the user paginates.
        for d in &media {
            if is_video_file(&d.name) {
                gallery_video_total += 1;
            } else {
                gallery_photo_total += 1;
            }
        }
        let offset = (page - 1) * per_page;
        if offset < media.len() {
            let end = (offset + per_page).min(media.len());
            gallery_slice = media[offset..end].to_vec();
        }
        media.len() as i64
    } else {
        0
    };

    // The star lookup always covers only the page; the tag lookup is narrowed
    // to the page too unless the sort-bar / tag filter needs the whole folder.
    let mut page_paths: Vec<String> = Vec::with_capacity(list_slice.len() + gallery_slice.len());
    for d in &list_slice {
        page_paths.push(entry_full_path(&path, &d.name));
    }
    for d in &gallery_slice {
        let full_path = entry_full_path(&path, &d.name);
        if !page_paths.contains(&full_path) {
            page_paths.push(full_path);
        }
    }

    if !needs_full_tags {
        let tag_details = state
            .repos
            .file_tag
            .find_tag_details_by_paths(&repo_id, &page_paths)
            .await?;
        (tags_by_path, folder_tags) = build_tags_map(tag_details);
    }

    // Query starred entries for the current page slice to stamp the `starred` field.
    let starred_set: HashSet<String> = repos
        .starred
        .find_by_user_repo_and_paths(user.user_id, &repo_id, &page_paths)
        .await?
        .into_iter()
        .map(|s| s.path.trim_end_matches('/').to_string())
        .collect();

    // Build FileEntry only for the current page (list/grid rows, then the
    // gallery media slice); both views share the same tag/star maps.
    let entries: Vec<FileEntry> = list_slice
        .iter()
        .map(|d| build_file_entry(&repo_id, &path, d, &starred_set, &tags_by_path))
        .collect();
    let gallery_groups: Vec<GalleryMonthGroup> = group_entries_by_month(
        t,
        gallery_slice
            .iter()
            .map(|d| build_file_entry(&repo_id, &path, d, &starred_set, &tags_by_path))
            .collect(),
    );

    // Gallery paginates the media subset independently (mtime-desc); expose
    // both the all-file counts (list/grid) and the media counts (gallery) to
    // the template so each view container carries its own pagination state.
    let gallery_has_more = page * per_page < gallery_total as usize;

    // The folder-level empty state applies to list/grid/all renders. In gallery-only
    // mode `entries` is intentionally empty (gallery is built from `gallery_groups`),
    // so gating on `entries.is_empty()` would wrongly show the empty-folder state for
    // every media folder; the gallery container renders its own "no photos or videos".
    let is_empty = render_view != "gallery" && entries.is_empty();

    // Build breadcrumb items from current_path.
    // Each item's path is relative (no leading /) for use in URL construction.
    let breadcrumbs = breadcrumbs_for(&path);

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
            is_empty,
            total,
            has_more,
            gallery_total,
            gallery_has_more,
            gallery_photo_total,
            gallery_video_total,
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
            is_empty,
            total,
            has_more,
            gallery_total,
            gallery_has_more,
            gallery_photo_total,
            gallery_video_total,
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

/// Characters left unencoded in a path segment (same set browsers leave alone).
const PATH_SAFE: &percent_encoding::AsciiSet = &percent_encoding::NON_ALPHANUMERIC
    .remove(b'-')
    .remove(b'_')
    .remove(b'.')
    .remove(b'~');

/// Build the unified `/repos/{repo}/files/{path}` content URL, percent-encoding
/// each path segment (matching the browser-side `encodeURIComponent` per segment).
fn content_url(repo_id: &str, path: &str, dl: bool) -> String {
    let encoded = encode_repo_path(path);
    let mut url = format!("/repos/{repo_id}/files/{encoded}");
    if dl {
        url.push_str("?dl=1");
    }
    url
}

/// Percent-encode a repository path for use in a URL, keeping the `/`
/// separators: every segment goes through [`PATH_SAFE`] individually.
fn encode_repo_path(path: &str) -> String {
    path.trim_start_matches('/')
        .split('/')
        .map(|seg| percent_encoding::utf8_percent_encode(seg, PATH_SAFE).to_string())
        .collect::<Vec<_>>()
        .join("/")
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
    let path = base::sanitize::safe_normalize_path(&path)
        .map_err(|e| AppError::BadRequest(format!("Invalid path: {e}")))?;
    let file_name = path.rsplit('/').next().unwrap_or("file").to_string();

    // ?dl=1 → force download. Content lives on the unified /repos/ endpoint,
    // which serves attachment disposition + ETag/304 caching.
    if query.dl.as_deref() == Some("1") {
        return Ok(Redirect::to(&content_url(&repo_id, &path, true)).into_response());
    }

    let is_video = is_video_file(&file_name);
    let is_audio = is_audio_file(&file_name);
    let is_image = is_preview_image_file(&file_name);
    let is_text = is_previewable_file(&file_name);

    if !is_video && !is_audio && !is_image && !is_text {
        // Non-preview binary files — redirect to the unified content endpoint.
        // The browser inlines/downloads per Content-Type exactly as before, but
        // /libraries/ itself never serves bytes.
        return Ok(Redirect::to(&content_url(&repo_id, &path, false)).into_response());
    }

    // Everything the three preview pages share. They render inside the normal
    // app shell (topbar + library tree), so this is only the page body.
    let ctx = crate::ui::ctx::build_page_ctx(&state, &user).await?;
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
    let parent_path = parent_path_from(&path).trim_start_matches('/').to_string();
    let content = content_url(&repo_id, &path, false);
    let download = content_url(&repo_id, &path, true);
    // The trail ends at the file, so the last crumb renders as the current page.
    let breadcrumbs = breadcrumbs_for(&path);
    let html = if is_video || is_audio {
        PreviewMediaTemplate {
            urls: ctx.urls,
            t: ctx.t,
            user_email: ctx.user_email,
            is_admin: ctx.is_admin,
            repo_name,
            file_name: file_name.clone(),
            repo_id: repo_id.clone(),
            content_url: content,
            download_url: download,
            parent_path,
            breadcrumbs,
            size_display,
            active_page: "repos",
            left_panel_repos: ctx.left_panel_repos,
            current_repo_id: Some(repo_id),
            is_video,
        }
        .render()
    } else if is_image {
        PreviewImageTemplate {
            urls: ctx.urls,
            t: ctx.t,
            user_email: ctx.user_email,
            is_admin: ctx.is_admin,
            repo_name,
            file_name: file_name.clone(),
            repo_id: repo_id.clone(),
            content_url: content,
            download_url: download,
            parent_path,
            breadcrumbs,
            size_display,
            active_page: "repos",
            left_panel_repos: ctx.left_panel_repos,
            current_repo_id: Some(repo_id),
        }
        .render()
    } else {
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
        PreviewTextTemplate {
            urls: ctx.urls,
            t: ctx.t,
            user_email: ctx.user_email,
            is_admin: ctx.is_admin,
            repo_name,
            file_name: file_name.clone(),
            content: String::from_utf8_lossy(&data).to_string(),
            repo_id: repo_id.clone(),
            content_url: content,
            download_url: download,
            parent_path,
            breadcrumbs,
            size_display,
            active_page: "repos",
            left_panel_repos: ctx.left_panel_repos,
            current_repo_id: Some(repo_id),
        }
        .render()
    }
    .map_err(|e| AppError::internal(e.to_string()))?;

    Ok(Html(html).into_response())
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
    // Case-insensitive: a phone camera writes `IMG_0001.MOV` as readily as
    // `.mp4`, and matching the raw name served those as
    // `application/octet-stream` — a header that tells every client the bytes
    // are neither media nor text, and that sits next to `nosniff`.
    let filename = filename.to_ascii_lowercase();
    let filename = filename.as_str();
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

    /// Extensions arrive in whatever case the camera chose; the content type is
    /// what tells a client whether the bytes are media, text or a download.
    #[test]
    fn mime_guess_ignores_extension_case() {
        assert_eq!(mime_guess("clip.mp4"), "video/mp4");
        assert_eq!(mime_guess("CLIP.MP4"), "video/mp4");
        assert_eq!(mime_guess("IMG_0001.MOV"), "video/quicktime");
        assert_eq!(mime_guess("PHOTO.JPG"), "image/jpeg");
        assert_eq!(mime_guess("notes.TXT"), "text/plain; charset=utf-8");
        // Unknown stays unknown rather than being guessed at.
        assert_eq!(mime_guess("archive.bin"), "application/octet-stream");
    }

    /// The singular keys exist so a freshly touched item does not read
    /// "1 days ago"; the 14-day cutoff hands the exact date to the locale's own
    /// date format (which is why zh gets no ISO dashes).
    #[test]
    fn relative_time_singulars_and_cutoff() {
        let t = I18n::get(None);
        let now = 1_800_000_000;
        assert_eq!(format_relative_time(t, now, now), "Just now");
        assert_eq!(format_relative_time(t, now, now - 1), "1 second ago");
        assert_eq!(format_relative_time(t, now, now - 2), "2 seconds ago");
        assert_eq!(format_relative_time(t, now, now - 60), "1 minute ago");
        assert_eq!(format_relative_time(t, now, now - 3600), "1 hour ago");
        assert_eq!(format_relative_time(t, now, now - 5 * 3600), "5 hours ago");
        assert_eq!(format_relative_time(t, now, now - 86400), "1 day ago");
        assert_eq!(format_relative_time(t, now, now - 3 * 86400), "3 days ago");
        assert_eq!(
            format_relative_time(t, now, now - 14 * 86400),
            "2027-01-01",
            ">= 14 days falls back to a date"
        );

        let zh = I18n::get(Some("zh"));
        assert_eq!(format_relative_time(zh, now, now - 86400), "1 天前");
        assert_eq!(
            format_relative_time(zh, now, now - 14 * 86400),
            "2027年01月01日"
        );
    }

    /// The trail is shared by the browser (a directory) and the full-page
    /// preview (a file, whose own name is the last crumb): each item carries the
    /// path up to and including itself, relative and URL-ready.
    #[test]
    fn breadcrumbs_accumulate_the_path() {
        assert!(breadcrumbs_for("/").is_empty());
        assert!(breadcrumbs_for("").is_empty());

        let trail = breadcrumbs_for("/Field Photos");
        assert_eq!(trail.len(), 1);
        assert_eq!(trail[0].label, "Field Photos");
        assert_eq!(trail[0].path, "Field Photos");

        let trail = breadcrumbs_for("/a/b/c.png");
        let as_pairs: Vec<(&str, &str)> = trail
            .iter()
            .map(|c| (c.label.as_str(), c.path.as_str()))
            .collect();
        assert_eq!(
            as_pairs,
            vec![("a", "a"), ("b", "a/b"), ("c.png", "a/b/c.png")]
        );

        // A trailing slash (directory URL) does not add an empty crumb.
        assert_eq!(breadcrumbs_for("/a/b/").len(), 2);
    }

    fn make_entry(name: &str, entry_type: &str, size: i64, mtime: i64) -> FileEntry {
        FileEntry {
            name: name.to_string(),
            entry_type: entry_type.to_string(),
            size,
            size_display: String::new(),
            mtime,
            icon_color: "",
            relative_path: String::new(),
            is_previewable: false,
            starred: false,
            extension: None,
            name_base: name.to_string(),
            ext_display: String::new(),
            tile_kind: if entry_type == "dir" { "dir" } else { "doc" },
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
