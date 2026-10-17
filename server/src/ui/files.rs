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

/// Largest page number the file browser accepts. `page * per_page` is a slice
/// offset, so it must stay far from `usize` overflow on 32-bit targets.
const MAX_BROWSER_PAGE: u32 = 10_000;

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
    /// Whether the *rendered view* has content. Gallery-only renders build from
    /// `gallery_groups`, so `entries` stays empty and must not decide this.
    pub is_empty: bool,
    pub total: i64,
    pub has_more: bool,
    /// Gallery paginates media separately (mtime-desc); these counts differ from
    /// the list/grid all-file counts.
    pub gallery_total: i64,
    pub gallery_has_more: bool,
    pub gallery_photo_total: i64,
    pub gallery_video_total: i64,
    pub page: u32,
    /// "all" renders all three views; "list"/"grid"/"gallery" render only that one.
    pub render_view: &'static str,
    pub active_page: &'static str,
    pub left_panel_repos: Vec<crate::service::repo::service::LeftPanelRepo>,
    pub current_repo_id: Option<String>,
    pub max_upload_size_mb: u64,
    pub sort_field: String,
    pub sort_order: String,
    pub gallery_groups: Vec<GalleryMonthGroup>,
    pub folder_tags: Vec<TagChip>,
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
    pub is_empty: bool,
    pub total: i64,
    pub has_more: bool,
    pub gallery_total: i64,
    pub gallery_has_more: bool,
    pub gallery_photo_total: i64,
    pub gallery_video_total: i64,
    pub page: u32,
    pub render_view: &'static str,
    pub csrf_token: String,
    pub sort_field: String,
    pub sort_order: String,
    pub gallery_groups: Vec<GalleryMonthGroup>,
    pub folder_tags: Vec<TagChip>,
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
    pub download_url: String,
    pub parent_path: String,
    /// Crumb trail ending at the file; the last item is the current page, not a link.
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
    pub content_url: String,
    pub download_url: String,
    pub parent_path: String,
    pub breadcrumbs: Vec<BreadcrumbItem>,
    pub size_display: String,
    pub active_page: &'static str,
    pub left_panel_repos: Vec<crate::service::repo::service::LeftPanelRepo>,
    pub current_repo_id: Option<String>,
}

/// Video/audio preview page: the `<video>`/`<audio>` `src` points at the unified
/// `/repos/...` content endpoint (Range-capable); `/libraries/...` never serves bytes.
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
    pub content_url: String,
    pub download_url: String,
    pub parent_path: String,
    pub breadcrumbs: Vec<BreadcrumbItem>,
    pub size_display: String,
    pub active_page: &'static str,
    pub left_panel_repos: Vec<crate::service::repo::service::LeftPanelRepo>,
    pub current_repo_id: Option<String>,
    pub is_video: bool,
}

#[derive(Clone)]
pub struct FileEntry {
    pub name: String,
    pub entry_type: String, // "file" or "dir"
    pub size: i64,
    pub size_display: String,
    pub mtime: i64,
    pub icon_color: &'static str,
    pub relative_path: String,
    pub is_previewable: bool,
    pub starred: bool,
    /// File extension in uppercase (e.g. "PDF", "PNG"), None for directories.
    pub extension: Option<String>,
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
    pub is_video: bool,
    pub is_audio: bool,
    pub modifier_email: String,
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
    pub label: String,
    /// Entries belonging to this month, sorted by mtime descending.
    pub entries: Vec<FileEntry>,
}

/// Returns true if the extension is a video one; gallery renders a play placeholder.
pub fn is_video_file(name: &str) -> bool {
    std::path::Path::new(name)
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| crate::thumbnail_util::is_video_ext(&e.to_ascii_lowercase()))
        .unwrap_or(false)
}

/// Returns true if the extension is an audio one; enables inline playback and covers.
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

/// Group already-sorted (mtime desc) entries by calendar month, newest first.
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

/// Sort entries: directories always first, then by field ("name"/"mtime"/"size").
pub fn sort_entries(entries: &mut [FileEntry], sort: &str, sort_order: &str) {
    entries.sort_by(|a, b| {
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

fn entry_full_path(path: &str, name: &str) -> String {
    if path == "/" {
        format!("/{name}")
    } else {
        format!("{}/{}", path.trim_end_matches('/'), name)
    }
}

/// Sort dirents exactly like `sort_entries` sorts `FileEntry`.
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

/// Whether a dirent belongs in the gallery contact sheet. Audio is deliberately
/// excluded; it stays reachable in the list and grid views.
fn is_gallery_media_dirent(e: &DirEntry) -> bool {
    e.entry_type == "file" && (is_video_file(&e.name) || is_thumbnail_image(&e.name))
}

/// Build the display `FileEntry` for one dirent; page slice only, so cost stays bounded.
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
    // Images get in-process thumbnails; audio/video via ffmpeg (frame or embedded
    // cover art). Audio without cover art falls back to a badge: the endpoint 404s.
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
    // How the grid/gallery draws this entry, decided once so templates stay free
    // of nested `if let` chains: "dir", "image", "video" or "doc".
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

/// Whether the thumbnail service decodes the extension: in-process images plus HEIC/HEIF/AVIF.
fn is_thumbnail_image(name: &str) -> bool {
    std::path::Path::new(name)
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| crate::thumbnail_util::is_thumbnail_image_ext(&e.to_lowercase()))
        .unwrap_or(false)
}

/// Every extension the Web UI opens in its image preview. The client decides
/// "image" a second time (see `QUICK_PREVIEW_IMAGE_EXTS` in `file-meta.js`), so
/// this list and `MIME_BY_EXT` are one fact, held by the tests below.
const PREVIEW_IMAGE_EXTS: &[&str] = &[
    "png", "jpg", "jpeg", "gif", "webp", "bmp", "svg", "tiff", "tif", "heic", "heif", "avif",
];

/// Extension → content type: the one place a served file's type comes from, used
/// by both `/repos/…` and WebDAV.
///
/// A missing row is not cosmetic: `nosniff` makes the content type the only thing
/// that lets an `<img>` paint. `.html` and the other markup formats are
/// `text/plain` on purpose — an uploaded document must never be served as a
/// document in this origin — while `.svg` *is* an image type, which is why its
/// response policy is `sandbox` (see `middleware::apply_security_headers`).
const MIME_BY_EXT: &[(&str, &str)] = &[
    ("png", "image/png"),
    ("jpg", "image/jpeg"),
    ("jpeg", "image/jpeg"),
    ("gif", "image/gif"),
    ("webp", "image/webp"),
    ("bmp", "image/bmp"),
    ("svg", "image/svg+xml"),
    ("ico", "image/x-icon"),
    ("tiff", "image/tiff"),
    ("tif", "image/tiff"),
    ("heic", "image/heic"),
    ("heif", "image/heif"),
    ("avif", "image/avif"),
    ("mp4", "video/mp4"),
    ("webm", "video/webm"),
    ("mov", "video/quicktime"),
    ("3gp", "video/3gpp"),
    ("mkv", "video/x-matroska"),
    ("avi", "video/x-msvideo"),
    ("wmv", "video/x-ms-wmv"),
    ("flv", "video/x-flv"),
    ("mp3", "audio/mpeg"),
    ("flac", "audio/flac"),
    ("wav", "audio/wav"),
    ("ogg", "audio/ogg"),
    ("opus", "audio/ogg"),
    ("m4a", "audio/mp4"),
    ("aac", "audio/aac"),
    ("wma", "audio/x-ms-wma"),
    ("pdf", "application/pdf"),
    ("txt", "text/plain; charset=utf-8"),
    ("md", "text/plain; charset=utf-8"),
    ("rs", "text/plain; charset=utf-8"),
    ("py", "text/plain; charset=utf-8"),
    ("js", "text/plain; charset=utf-8"),
    ("ts", "text/plain; charset=utf-8"),
    ("html", "text/plain; charset=utf-8"),
    ("css", "text/plain; charset=utf-8"),
    ("go", "text/plain; charset=utf-8"),
    ("java", "text/plain; charset=utf-8"),
    ("c", "text/plain; charset=utf-8"),
    ("cpp", "text/plain; charset=utf-8"),
    ("h", "text/plain; charset=utf-8"),
    ("rb", "text/plain; charset=utf-8"),
    ("php", "text/plain; charset=utf-8"),
    ("sh", "text/plain; charset=utf-8"),
    ("toml", "text/plain; charset=utf-8"),
    ("json", "text/plain; charset=utf-8"),
    ("yaml", "text/plain; charset=utf-8"),
    ("yml", "text/plain; charset=utf-8"),
    ("csv", "text/plain; charset=utf-8"),
    ("xml", "text/plain; charset=utf-8"),
    ("sql", "text/plain; charset=utf-8"),
    ("conf", "text/plain; charset=utf-8"),
    ("ini", "text/plain; charset=utf-8"),
    ("log", "text/plain; charset=utf-8"),
];

const OCTET_STREAM: &str = "application/octet-stream";

fn ext_of(name: &str) -> Option<&str> {
    std::path::Path::new(name)
        .extension()
        .and_then(|e| e.to_str())
}

/// A lookup, case-insensitively: a camera writes `IMG_0001.SVG` as readily as
/// `.svg`. `None` for anything unmapped.
fn mime_of(ext: &str) -> Option<&'static str> {
    MIME_BY_EXT
        .iter()
        .find(|(known, _)| known.eq_ignore_ascii_case(ext))
        .map(|(_, mime)| *mime)
}

/// Returns true for still images the browser can render in the full-page preview:
/// SVG (browser-native, no thumbnail) plus the ffmpeg-only formats. Case-insensitive.
fn is_preview_image_file(name: &str) -> bool {
    ext_of(name).is_some_and(|ext| {
        PREVIEW_IMAGE_EXTS
            .iter()
            .any(|known| known.eq_ignore_ascii_case(ext))
    })
}

/// Whether the browser opens a file in the code block (or image preview) instead
/// of handing the bytes over: one fact read off `MIME_BY_EXT`, not a second list
/// that can drift. A hidden file with no extension of its own is not previewable.
pub fn is_previewable_file(name: &str) -> bool {
    is_preview_image_file(name) || mime_guess(name).starts_with("text/")
}

/// Colour class for the file-type badge: colour is reserved for semantic state, so
/// file types carry no per-kind hue; `build.rs` still scans these strings for Tailwind.
fn file_icon_color(_name: &str) -> &'static str {
    "text-ink-2"
}

/// Split a display name into its stem and a lowercase dotted suffix, keeping
/// dotfiles whole (`.bashrc` is a name, not a stem); see `file_extension` for the rules.
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

fn file_extension(name: &str) -> Option<String> {
    let (_, ext) = name.rsplit_once('.')?;
    if ext.is_empty() || ext.contains('/') {
        return None;
    }
    Some(ext.to_uppercase())
}

pub use super::format_size;

#[derive(Clone)]
pub struct BreadcrumbItem {
    pub label: String,
    pub path: String,
}

/// Crumb trail for a repo path, each item carrying the path up to and including
/// itself (relative, no leading `/`, so it drops straight into a URL).
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

#[derive(Clone, Deserialize)]
pub struct FileBrowserQuery {
    pub partial: Option<String>,
    pub dl: Option<String>,
    pub view: Option<String>,
    pub page: Option<u32>,
    pub per_page: Option<u32>,
    pub sort: Option<String>,       // "name" | "mtime" | "size"
    pub sort_order: Option<String>, // "asc" | "desc"
    pub tag: Option<String>,
}

async fn verify_repo_access(
    member_repo: &dyn crate::repository::member::MemberRepository,
    user_id: i32,
    repo_id: &str,
) -> Result<(), AppError> {
    crate::domain::permission::check_repo_read_permission(member_repo, repo_id, user_id).await?;
    Ok(())
}

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
/// The desktop client's "view on website" builds `/library/<repo-id>/<name>/`
/// (`seafile-client/src/ui/repo-tree-view.cpp:578`) and only the id is meaningful.
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

/// Build the `/libraries/{id}/files/…` redirect, refusing anything that could not
/// have come from a browser navigation.
///
/// Id and path are both percent-decoded, so a `%0d%0a` must not be echoed into
/// `Location`: the id is restricted to the alphabet repository ids use, and the
/// path is normalized (rejecting `..`, NUL) and re-encoded per segment.
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

    let repo_record = repos
        .repo
        .find_by_id(&repo_id)
        .await?
        .ok_or_else(|| AppError::NotFound("Repository not found".to_string()))?;

    // Media / image / text extensions are unambiguously files: serve the preview page
    // directly; if the name is a directory the preview errors and we fall back.
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
                // Not a file — fall through to the directory listing.
            }
        }
    }

    // List directory entries from the FS object tree; a path that is a file falls
    // through to file serving. For root `/`, errors mean an empty repo.
    let entries_result =
        crate::service::fs::dir::list_dir_from_fs_tree(repos, &repo_id, &path).await;

    let entries_data = match entries_result {
        Ok(data) => data,
        Err(_e) if path == "/" => (String::new(), vec![]),
        Err(AppError::NotFound(_)) => {
            return serve_file(user, state, repo_id, path, query)
                .await
                .map(IntoResponse::into_response);
        }
        Err(e) => {
            // Database errors, I/O errors — do NOT mask these as 500 Internal.
            return Err(e);
        }
    };

    // Pagination state is needed before the tag/star lookup width can be decided.
    // Both bounds are clamped: `page` is multiplied by `per_page`, so an unbounded
    // value is one request away from an overflow (panic in debug, huge OFFSET).
    let per_page = query.per_page.unwrap_or(200).clamp(1, 500) as usize;
    let page = query.page.unwrap_or(1).clamp(1, MAX_BROWSER_PAGE) as usize;

    // Full-page loads render all three views so view switching is a pure client-side
    // class toggle. Partial reloads (pagination, post-mutation) render only one view.
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

    let sort_field = query.sort.as_deref().unwrap_or("name");
    let sort_order = query.sort_order.as_deref().unwrap_or("asc");

    let has_tag_filter = query.tag.as_deref().map(|t| !t.is_empty()).unwrap_or(false);

    // Tag lookups cover the whole folder when the sort-bar needs its full tag set
    // (`view=all`) or a filter must scan before paginating; partials narrow to the page.
    let needs_full_tags = has_tag_filter || render_view == "all";

    // Sort/filter/paginate on lightweight dirents first, then build `FileEntry`
    // only for the page slice: cost tracks page size, not folder size.
    let mut dirents = entries_data.1;

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

    // Gallery: media dirents paginate on their own, always mtime-desc.
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

    // The star lookup covers only the page; so does the tag lookup unless it needs the folder.
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

    let starred_set: HashSet<String> = repos
        .starred
        .find_by_user_repo_and_paths(user.user_id, &repo_id, &page_paths)
        .await?
        .into_iter()
        .map(|s| s.path.trim_end_matches('/').to_string())
        .collect();

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

    // Each view container carries its own counts: all-file (list/grid) and media (gallery).
    let gallery_has_more = page * per_page < gallery_total as usize;

    // The folder-level empty state applies to list/grid/all renders only: in
    // gallery-only mode `entries` is empty by design.
    let is_empty = render_view != "gallery" && entries.is_empty();

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
            max_upload_size_mb: state.config().server.max_upload_size_mb,
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

/// Build the unified `/repos/{repo}/files/{path}` content URL, percent-encoding each segment.
fn content_url(repo_id: &str, path: &str, dl: bool) -> String {
    let encoded = encode_repo_path(path);
    let mut url = format!("/repos/{repo_id}/files/{encoded}");
    if dl {
        url.push_str("?dl=1");
    }
    url
}

/// Percent-encode a repository path for a URL, keeping the `/` separators.
fn encode_repo_path(path: &str) -> String {
    path.trim_start_matches('/')
        .split('/')
        .map(|seg| percent_encoding::utf8_percent_encode(seg, PATH_SAFE).to_string())
        .collect::<Vec<_>>()
        .join("/")
}

/// Serve a file directly from the repo (preview or download).
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
        // Non-preview binary files redirect to the unified content endpoint: the
        // browser inlines/downloads per Content-Type, `/libraries/` serves no bytes.
        return Ok(Redirect::to(&content_url(&repo_id, &path, false)).into_response());
    }

    // Page context shared by the three preview pages, which render inside the app shell.
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

/// The content type a served file is sent with: a case-insensitive [`MIME_BY_EXT`]
/// lookup, falling back to `application/octet-stream` next to `nosniff`.
pub(crate) fn mime_guess(filename: &str) -> &'static str {
    ext_of(filename).and_then(mime_of).unwrap_or(OCTET_STREAM)
}

fn urlencode_path(path: &str) -> String {
    percent_encoding::utf8_percent_encode(path, percent_encoding::NON_ALPHANUMERIC).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mime_guess_ignores_extension_case() {
        assert_eq!(mime_guess("clip.mp4"), "video/mp4");
        assert_eq!(mime_guess("CLIP.MP4"), "video/mp4");
        assert_eq!(mime_guess("IMG_0001.MOV"), "video/quicktime");
        assert_eq!(mime_guess("PHOTO.JPG"), "image/jpeg");
        assert_eq!(mime_guess("logo.SVG"), "image/svg+xml");
        assert_eq!(mime_guess("scan.BMP"), "image/bmp");
        assert_eq!(mime_guess("shot.WEBP"), "image/webp");
        assert_eq!(mime_guess("notes.TXT"), "text/plain; charset=utf-8");
        assert_eq!(mime_guess("archive.bin"), "application/octet-stream");
    }

    /// A previewable image must arrive as `image/*`: the preview is an `<img>` and
    /// `nosniff` makes the content type the only thing that lets it paint.
    #[test]
    fn every_preview_image_has_an_image_type() {
        for ext in PREVIEW_IMAGE_EXTS {
            let upper = ext.to_uppercase();
            for name in [format!("photo.{ext}"), format!("PHOTO.{upper}")] {
                let mime = mime_guess(&name);
                assert!(mime.starts_with("image/"), "{name} is served as {mime}");
            }
        }
        assert_eq!(mime_guess("logo.svg"), "image/svg+xml");
        assert_eq!(mime_guess("scan.bmp"), "image/bmp");
        assert_eq!(mime_guess("shot.webp"), "image/webp");
    }

    /// The table is the only map, so one extension must not carry two answers.
    #[test]
    fn the_mime_table_has_no_duplicate_extension() {
        let mut seen = std::collections::BTreeSet::new();
        for (ext, _) in MIME_BY_EXT {
            assert!(seen.insert(*ext), "{ext} is listed twice");
        }
    }

    /// An uploaded document is never served as a document — that would be same-origin markup.
    #[test]
    fn markup_is_served_as_text() {
        assert_eq!(mime_guess("page.html"), "text/plain; charset=utf-8");
        assert_eq!(mime_guess("feed.xml"), "text/plain; charset=utf-8");
        assert_eq!(mime_guess("script.js"), "text/plain; charset=utf-8");
    }

    /// Text is previewable because it is served as text — one fact, not two lists.
    #[test]
    fn previewable_text_is_the_text_the_server_serves() {
        for name in ["notes.log", "data.csv", "main.go", "app.ts", "pkg.ini"] {
            assert!(
                is_previewable_file(name),
                "{name} should open the code block"
            );
        }
        for name in ["archive.bin", "report.pdf", "icon.ico", "noext", ".txt"] {
            assert!(
                !is_previewable_file(name),
                "{name} has no previewable extension"
            );
        }
    }

    /// The quick-preview dialog decides "image" a second time in JavaScript: keep both in step.
    #[test]
    fn the_quick_preview_image_list_matches() {
        let js = include_str!("../../frontend/core/file-meta.js");
        let line = js
            .lines()
            .find(|line| line.contains("QUICK_PREVIEW_IMAGE_EXTS = ["))
            .expect("file-meta.js no longer declares QUICK_PREVIEW_IMAGE_EXTS");
        let list: Vec<&str> = line
            .split_once('[')
            .and_then(|(_, rest)| rest.split_once(']'))
            .expect("the declaration is not a single-line array")
            .0
            .split(',')
            .map(|item| item.trim().trim_matches('"'))
            .filter(|item| !item.is_empty())
            .collect();
        assert_eq!(list.as_slice(), PREVIEW_IMAGE_EXTS);
    }

    /// Each crumb carries the path up to and including itself; see `breadcrumbs_for`.
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
