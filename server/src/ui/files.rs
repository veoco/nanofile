/// Web UI file browser handlers.
use askama::Template;
use axum::{
    Form,
    extract::{Path, Query, State},
    http::{StatusCode, header},
    response::{Html, IntoResponse},
};
use chrono::Utc;
use sea_orm::{ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter};
use serde::Deserialize;
use std::collections::HashSet;
use std::sync::Arc;

use crate::AppState;
use crate::activity_log;
use crate::entity::{commit, repo};
use crate::error::AppError;
use crate::repo::download::Downloader;
use crate::repo::file_ops::FileOps;
use crate::repo::resolve_fs_id;
use crate::serialization::S_IFDIR;
use crate::serialization::fs_json::{DirEntryData, FsDirData, SEAF_METADATA_TYPE_DIR};

use super::auth_extractor::WebUser;

// ─── Templates ───────────────────────────────────────────────────────────────

#[derive(Template)]
#[template(path = "files/browser.html")]
pub struct FileBrowserTemplate {
    pub urls: &'static crate::static_assets::TemplateUrls,
    pub user_email: String,
    pub is_admin: bool,
    pub session_token: String,
    pub csrf_token: String,
    pub repo_id: String,
    pub repo_name: String,
    pub current_path: String,
    pub breadcrumbs: Vec<BreadcrumbItem>,
    pub entries: Vec<FileEntry>,
    pub active_page: &'static str,
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
    pub session_token: String,
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
}

// ─── Data types ──────────────────────────────────────────────────────────────

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
}

#[derive(Deserialize)]
pub struct ViewLibFileQuery {
    pub raw: Option<String>,
    pub dl: Option<String>,
}

#[derive(Deserialize)]
pub struct UploadForm {
    pub parent_dir: Option<String>,
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

/// GET /library/{id}/{*path} — Seahub-compatible file browser.
///
/// The URL format is /library/{repo_id}/{repo_name}/{*path} where
/// {repo_name} is purely cosmetic (ignored by the handler).
pub async fn file_browser_seahub(
    user: WebUser,
    State(state): State<Arc<AppState>>,
    Path((repo_id, path)): Path<(String, String)>,
    Query(query): Query<FileBrowserQuery>,
) -> Result<impl IntoResponse, AppError> {
    // Strip the cosmetic repo name from the path
    let cleaned = if path.is_empty() {
        "/".to_string()
    } else {
        let segments: Vec<&str> = path.split('/').collect();
        if segments.len() <= 1 {
            // Just the repo name, no real path — show root
            "/".to_string()
        } else {
            format!("/{}", segments[1..].join("/"))
        }
    };
    file_browser_inner(user, state, repo_id, cleaned, query).await
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

    // List directory entries from the FS object tree (authoritative source).
    let entries_data =
        crate::fs::handler::dir::list_dir_from_fs_tree(db, repos, &repo_id, &path).await?;

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

    if is_partial {
        let tpl = FileBrowserCoreTemplate {
            urls: crate::static_assets::template_urls(),
            repo_name: repo_record.name.clone(),
            repo_id: repo_id.clone(),
            current_path: path.clone(),
            breadcrumbs: breadcrumbs.clone(),
            entries,
            session_token: user.session_token.clone(),
            csrf_token,
        };
        let html = tpl
            .render()
            .map_err(|e| AppError::internal(e.to_string()))?;
        Ok(Html(html).into_response())
    } else {
        let tpl = FileBrowserTemplate {
            urls: crate::static_assets::template_urls(),
            user_email: user.email,
            is_admin: user.is_admin,
            session_token: user.session_token.clone(),
            csrf_token,
            repo_id,
            repo_name: repo_record.name,
            current_path: path,
            breadcrumbs,
            entries,
            active_page: "repos",
        };
        let html = tpl
            .render()
            .map_err(|e| AppError::internal(e.to_string()))?;
        Ok(Html(html).into_response())
    }
}

/// Ensure all parent directories exist for a given path, creating any that
/// are missing. Each missing level creates a commit so that `walk_up_ancestors`
/// (which resolves intermediate directories from HEAD) can always find them.
async fn ensure_parent_dirs(
    db: &DatabaseConnection,
    repo_id: &str,
    parent_path: &str,
    modifier: &str,
) -> Result<(), AppError> {
    if parent_path == "/" {
        return Ok(());
    }

    let segments: Vec<&str> = parent_path
        .trim_start_matches('/')
        .split('/')
        .filter(|s| !s.is_empty())
        .collect();
    if segments.is_empty() {
        return Ok(());
    }

    let mut root_fs_id = get_head_root_id(db, repo_id).await?;
    let mut current_path = String::from("/");
    let now = Utc::now().timestamp();

    for segment in &segments {
        let child_path = if current_path == "/" {
            format!("/{}", segment)
        } else {
            format!("{}/{}", current_path, segment)
        };

        // Skip if this directory already exists in the current tree
        if resolve_fs_id(db, repo_id, &root_fs_id, &child_path)
            .await
            .is_ok()
        {
            current_path = child_path;
            continue;
        }

        // Get the parent fs_id from the current root tree
        let parent_fs_id = if current_path == "/" {
            root_fs_id.clone()
        } else {
            resolve_fs_id(db, repo_id, &root_fs_id, &current_path)
                .await
                .map_err(|e| AppError::Internal(format!("resolve parent dir failed: {e}")))?
        };

        let seg = segment.to_string();
        let mod_email = modifier.to_string();
        // Clone for the modifier parameter so the original can be moved
        // into the closure.
        let mod_email_param = mod_email.clone();

        // Use the COMMIT variant — each level creates a commit so
        // walk_up_ancestors (which resolves ancestors from HEAD via
        // resolve_root_fs_id) sees the correct intermediate tree.
        root_fs_id = FileOps::update_dir_tree_and_commit(
            db,
            repo_id,
            &current_path,
            &parent_fs_id,
            &mod_email_param,
            &format!("Created directory {}", seg),
            crate::repo::file_ops::EMPTY_ANCESTOR_CHAIN,
            move |dirents| {
                if !dirents.iter().any(|d| d.name == seg) {
                    dirents.push(DirEntryData {
                        id: "0000000000000000000000000000000000000000".to_string(),
                        mode: S_IFDIR,
                        modifier: mod_email,
                        mtime: now,
                        name: seg,
                        size: 0,
                    });
                }
                Ok(())
            },
        )
        .await
        .map_err(|e| AppError::Internal(format!("mkdir failed: {e}")))?;

        current_path = child_path;
    }

    Ok(())
}

/// POST /library/{id}/upload — upload one or more files.
///
/// Supports standard form submit and AJAX (xhr=1) uploads.
/// The `multiple` and `webkitdirectory` attributes on the `<input>` allow
/// batch and folder selection; AJAX uploads send files individually to
/// preserve directory structure.
pub async fn upload_file(
    user: WebUser,
    State(state): State<Arc<AppState>>,
    Path(repo_id): Path<String>,
    mut multipart: axum::extract::Multipart,
) -> Result<impl IntoResponse, AppError> {
    let db = state.db.as_ref();
    crate::storage::check_repo_write_permission(db, &repo_id, user.user_id).await?;

    let mut parent_dir = String::from("/");
    let mut upload_repo_name = String::new();
    let mut is_xhr = false;
    let mut uploaded_count: u32 = 0;

    // First pass: collect all non-file metadata fields.
    // Multipart fields may arrive in any order, so we collect them first
    // to avoid processing a file before its parent_dir is known.
    let mut file_field: Option<(String, Vec<u8>)> = None;

    while let Some(field) = multipart
        .next_field()
        .await
        .map_err(|e| AppError::BadRequest(e.to_string()))?
    {
        let name = field.name().unwrap_or("").to_string();
        match name.as_str() {
            "parent_dir" => {
                parent_dir = field.text().await.unwrap_or_else(|_| "/".to_string());
            }
            "repo_name" => {
                upload_repo_name = field.text().await.unwrap_or_default();
            }
            "xhr" => {
                is_xhr = true;
            }
            "file" => {
                let file_name = field.file_name().unwrap_or("unnamed").to_string();
                let data = field
                    .bytes()
                    .await
                    .map_err(|e| AppError::BadRequest(e.to_string()))?
                    .to_vec();
                // Collect file data — will be processed after all metadata
                // fields are parsed, so parent_dir is guaranteed to be set.
                file_field = Some((file_name, data));
            }
            _ => {}
        }
    }

    // Second pass: process the collected file (if any).
    if let Some((file_name, data)) = file_field {
        // Validate filename (reject path separators and invalid characters)
        crate::sanitize::validate_filename(&file_name)
            .map_err(|e| AppError::BadRequest(format!("invalid filename: {e}")))?;

        // Ensure parent directory exists (creates intermediate dirs)
        ensure_parent_dirs(db, &repo_id, &parent_dir, &user.email).await?;

        // Get old file size for incremental size adjustment (0 if new file).
        let p = if parent_dir == "/" {
            format!("/{}", file_name)
        } else {
            format!("{}/{}", parent_dir, file_name)
        };
        let old_size = crate::repo::get_entry_total_size(db, &repo_id, &p)
            .await
            .ok()
            .unwrap_or(0);

        FileOps::create_file(
            db,
            &repo_id,
            &parent_dir,
            &file_name,
            &data,
            &user.email,
            true,
            &state.block_store,
            None,
        )
        .await
        .map_err(|e| AppError::internal(format!("upload failed: {e}")))?;

        // Log activity
        let op_type = if old_size > 0 { "edit" } else { "create" };
        activity_log::log_activity(
            db,
            &repo_id,
            op_type,
            "file",
            &p,
            user.user_id,
            None,
            None,
            None,
            None,
            None,
        )
        .await;

        // Adjust repo size (delta = new_size - old_size).
        let delta = data.len() as i64 - old_size;
        crate::repo::adjust_repo_size(db, &repo_id, delta).await?;

        uploaded_count = 1;
    }

    // AJAX uploads return a lightweight JSON response (no redirect).
    if is_xhr {
        let json = format!(r#"{{"success":true,"count":{uploaded_count}}}"#);
        return Ok((StatusCode::OK, [("Content-Type", "application/json")], json).into_response());
    }

    // Redirect back to the upload directory (standard form submit).
    let redirect = if parent_dir == "/" || upload_repo_name.is_empty() {
        format!("/library/{}/", repo_id)
    } else {
        let dir_path = parent_dir.trim_start_matches('/');
        format!("/library/{}/{}/{}", repo_id, upload_repo_name, dir_path)
    };
    Ok((StatusCode::FOUND, [("Location", &redirect)]).into_response())
}

/// GET /library/{id}/download/{*path} — download a file.
pub async fn download_file(
    user: WebUser,
    State(state): State<Arc<AppState>>,
    Path((repo_id, path)): Path<(String, String)>,
) -> Result<impl IntoResponse, AppError> {
    let db = state.db.as_ref();
    verify_repo_access(db, user.user_id, &repo_id).await?;

    let path = normalize_path(&path);
    let data = Downloader::download_file(db, &repo_id, &path, &state.block_store, None)
        .await
        .map_err(|e| AppError::Internal(format!("download failed: {e}")))?;

    let file_name = path.rsplit('/').next().unwrap_or("file");
    let content_type = mime_guess(file_name);

    Ok((StatusCode::OK, [("Content-Type", content_type)], data))
}

/// POST /library/{id}/delete — delete a file or directory.
pub async fn delete_entry(
    user: WebUser,
    State(state): State<Arc<AppState>>,
    Path(repo_id): Path<String>,
    Form(form): Form<std::collections::HashMap<String, String>>,
) -> Result<impl IntoResponse, AppError> {
    crate::auth::csrf::check_form_csrf(
        &state,
        &user.session_token,
        form.get("csrf_token").map(|s| s.as_str()),
    )?;
    let db = state.db.as_ref();
    crate::storage::check_repo_write_permission(db, &repo_id, user.user_id).await?;

    let path = form.get("p").map(|s| s.as_str()).unwrap_or("/");
    let path = normalize_path(path);
    let parent_path = parent_path_from(&path);

    // Derive the entry name from the path.
    let name = path.rsplit_once('/').map(|(_, n)| n).unwrap_or("");

    // Get the size of the entry being deleted (for repo size adjustment).
    let deleted_size = crate::repo::get_entry_total_size(db, &repo_id, &path)
        .await
        .ok()
        .unwrap_or(0);

    // Get root fs_id from the repo's head commit to resolve paths.
    let head_root_id = get_head_root_id(db, &repo_id).await?;

    // Resolve the parent directory's fs_id from the FS tree.
    let parent_fs_id = crate::repo::resolve_fs_id(db, &repo_id, &head_root_id, parent_path)
        .await
        .map_err(|e| AppError::Internal(format!("resolve parent failed: {e}")))?;

    // Best-effort trash recording (read entry details before tree update).
    // Fetch the actual head commit ID for accurate parent reference.
    let head_commit_id = state
        .repos
        .repo
        .find_by_id(&repo_id)
        .await
        .ok()
        .flatten()
        .and_then(|r| r.head_commit_id)
        .unwrap_or_default();
    if !head_commit_id.is_empty() {
        record_trash_best_effort(
            db,
            &repo_id,
            &parent_fs_id,
            &path,
            name,
            &head_commit_id,
            &user.email,
        )
        .await;
    }

    // Update the FS tree and create a commit
    FileOps::update_dir_tree_and_commit(
        db,
        &repo_id,
        parent_path,
        &parent_fs_id,
        &user.email,
        &format!("Deleted {}", name),
        crate::repo::file_ops::EMPTY_ANCESTOR_CHAIN,
        |dirents| {
            dirents.retain(|d| d.name != name);
            Ok(())
        },
    )
    .await
    .map_err(|e| AppError::Internal(e.to_string()))?;

    // Adjust repo size (subtract the deleted entry's size).
    crate::repo::adjust_repo_size(db, &repo_id, -deleted_size).await?;

    // Log activity
    // For obj_type, check if the name has extension or form provides context.
    // Default to "file" since most web UI deletes are files.
    activity_log::log_activity(
        db,
        &repo_id,
        "delete",
        "file",
        &path,
        user.user_id,
        None,
        None,
        None,
        None,
        None,
    )
    .await;

    // Redirect back to the current directory.
    let repo_name = form.get("repo_name").map(|s| s.as_str()).unwrap_or("");
    let current_dir = form.get("current_dir").map(|s| s.as_str()).unwrap_or("");
    let redirect = if !current_dir.is_empty() && !repo_name.is_empty() {
        // Use current_dir directly (it's relative, no leading slash)
        format!(
            "/library/{}/{}/{}",
            repo_id,
            repo_name,
            current_dir.trim_start_matches('/')
        )
    } else {
        format!("/library/{}/", repo_id)
    };
    Ok((StatusCode::FOUND, [("Location", &redirect)]).into_response())
}

/// POST /library/{id}/new-dir — create a directory.
pub async fn create_directory(
    user: WebUser,
    State(state): State<Arc<AppState>>,
    Path(repo_id): Path<String>,
    Form(form): Form<std::collections::HashMap<String, String>>,
) -> Result<impl IntoResponse, AppError> {
    crate::auth::csrf::check_form_csrf(
        &state,
        &user.session_token,
        form.get("csrf_token").map(|s| s.as_str()),
    )?;
    let db = state.db.as_ref();
    crate::storage::check_repo_write_permission(db, &repo_id, user.user_id).await?;

    let path = form.get("p").map(|s| s.as_str()).unwrap_or("/new_folder");
    let path = normalize_path(path);

    let name = path.rsplit('/').next().unwrap_or("new_folder").to_string();

    let parts: Vec<&str> = path.trim_start_matches('/').split('/').collect();
    let parent_path = if parts.len() > 1 {
        format!("/{}", parts[..parts.len() - 1].join("/"))
    } else {
        "/".to_string()
    };

    // Use EMPTY_SHA1 sentinel for empty directories, matching the API path.
    // The seafile protocol uses all-zeros as a well-known sentinel meaning
    // "empty directory". The C client's diff engine specifically checks for
    // this sentinel to generate DIR_ADDED entries — using a real SHA1 would
    // silently drop the entry during diff expansion and the directory would
    // never be created on the client filesystem.
    let dir_fs_id = "0000000000000000000000000000000000000000".to_string();
    // No fs_object record needed — EMPTY_SHA1 is a well-known sentinel
    // handled by read_fs_dir_data() and the seafile client natively.

    // Find parent directory's fs_id via the head commit's FS tree
    let parent_fs_id = if parent_path == "/" {
        // Root-level directory — resolve or create root
        match get_head_root_id(db, &repo_id).await {
            Ok(root_id) => root_id,
            Err(_) => {
                // Empty repo — create root fs_object
                let root_dir = FsDirData {
                    dirents: vec![],
                    obj_type: SEAF_METADATA_TYPE_DIR,
                    version: 1,
                };

                root_dir
                    .compute_and_store(db, &repo_id)
                    .await
                    .map_err(|e| AppError::Internal(e.to_string()))?
            }
        }
    } else {
        let head_root_id = get_head_root_id(db, &repo_id).await?;
        crate::repo::resolve_fs_id(db, &repo_id, &head_root_id, &parent_path)
            .await
            .map_err(|e| AppError::Internal(format!("resolve parent failed: {e}")))?
    };

    // Use update_dir_tree_and_commit to add the new directory entry to the parent
    let now = chrono::Utc::now().timestamp();
    FileOps::update_dir_tree_and_commit(
        db,
        &repo_id,
        &parent_path,
        &parent_fs_id,
        &user.email,
        &format!("Created directory {}", name),
        crate::repo::file_ops::EMPTY_ANCESTOR_CHAIN,
        |dirents| {
            if !dirents.iter().any(|d| d.name == name) {
                dirents.push(DirEntryData {
                    id: dir_fs_id.clone(),
                    mode: crate::serialization::S_IFDIR,
                    modifier: user.email.clone(),
                    mtime: now,
                    name: name.clone(),
                    size: 0,
                });
            }
            Ok(())
        },
    )
    .await
    .map_err(|e| AppError::Internal(e.to_string()))?;

    // Log activity
    activity_log::log_activity(
        db,
        &repo_id,
        "create",
        "dir",
        &path,
        user.user_id,
        None,
        None,
        None,
        None,
        None,
    )
    .await;

    // Redirect back to the parent directory.
    let repo_name = form.get("repo_name").map(|s| s.as_str()).unwrap_or("");
    let current_dir = form.get("current_dir").map(|s| s.as_str()).unwrap_or("");
    let redirect = if !current_dir.is_empty() && !repo_name.is_empty() {
        let dir_path = current_dir.trim_start_matches('/');
        if dir_path.is_empty() {
            format!("/library/{}/{}/", repo_id, repo_name)
        } else {
            format!("/library/{}/{}/{}", repo_id, repo_name, dir_path)
        }
    } else {
        format!("/library/{}/", repo_id)
    };
    Ok((StatusCode::FOUND, [("Location", &redirect)]).into_response())
}

/// POST /library/{id}/rename — rename a file or directory.
pub async fn rename_entry(
    user: WebUser,
    State(state): State<Arc<AppState>>,
    Path(repo_id): Path<String>,
    Form(form): Form<std::collections::HashMap<String, String>>,
) -> Result<impl IntoResponse, AppError> {
    crate::auth::csrf::check_form_csrf(
        &state,
        &user.session_token,
        form.get("csrf_token").map(|s| s.as_str()),
    )?;
    let db = state.db.as_ref();
    crate::storage::check_repo_write_permission(db, &repo_id, user.user_id).await?;

    let path = form.get("p").map(|s| s.as_str()).unwrap_or("");
    let new_name = form.get("new_name").map(|s| s.as_str()).unwrap_or("");

    if path.is_empty() || new_name.is_empty() {
        return Err(AppError::BadRequest(
            "path and new_name are required".to_string(),
        ));
    }

    let parent_path = parent_path_from(path);
    let old_name = path.rsplit_once('/').map(|(_, n)| n).unwrap_or("");

    // Get root fs_id from head commit
    let head_root_id = get_head_root_id(db, &repo_id).await?;

    // Get parent's current fs_id via FS tree resolution
    let parent_fs_id = crate::repo::resolve_fs_id(db, &repo_id, &head_root_id, parent_path)
        .await
        .map_err(|e| AppError::Internal(format!("resolve parent failed: {e}")))?;

    // Read parent's FsDirData to find the child's fs_id
    let parent_data = crate::repo::read_fs_dir_data(db, &repo_id, &parent_fs_id)
        .await
        .map_err(|e| AppError::Internal(format!("read parent failed: {e}")))?;
    let child_id = parent_data
        .dirents
        .iter()
        .find(|d| d.name == old_name)
        .map(|d| d.id.clone())
        .ok_or_else(|| AppError::NotFound("entry not found".to_string()))?;

    let entry_type_label = if parent_data
        .dirents
        .iter()
        .any(|d| d.id == child_id && d.mode == crate::serialization::S_IFDIR)
    {
        "directory"
    } else {
        "file"
    };

    // Update the FS tree and create a commit
    FileOps::update_dir_tree_and_commit(
        db,
        &repo_id,
        parent_path,
        &parent_fs_id,
        &user.email,
        &format!("Renamed {} {}", entry_type_label, old_name),
        crate::repo::file_ops::EMPTY_ANCESTOR_CHAIN,
        |dirents| {
            if let Some(d) = dirents.iter_mut().find(|d| d.id == child_id) {
                d.name = new_name.to_string();
            }
            Ok(())
        },
    )
    .await
    .map_err(|e| AppError::Internal(e.to_string()))?;

    // Log activity
    let new_path = if parent_path == "/" {
        format!("/{}", new_name)
    } else {
        format!("{}/{}", parent_path, new_name)
    };
    let obj_type = if entry_type_label == "directory" {
        "dir"
    } else {
        "file"
    };
    activity_log::log_activity(
        db,
        &repo_id,
        "rename",
        obj_type,
        &new_path,
        user.user_id,
        Some(path),
        None,
        None,
        None,
        None,
    )
    .await;

    // Redirect back to current directory.
    let current_dir = form.get("current_dir").map(|s| s.as_str()).unwrap_or("");
    let repo_name = form.get("repo_name").map(|s| s.as_str()).unwrap_or("");
    let redirect = if !current_dir.is_empty() && !repo_name.is_empty() {
        let dir_path = current_dir.trim_start_matches('/');
        if dir_path.is_empty() {
            format!("/library/{}/{}/", repo_id, repo_name)
        } else {
            format!("/library/{}/{}/{}", repo_id, repo_name, dir_path)
        }
    } else {
        format!("/library/{}/", repo_id)
    };
    Ok((StatusCode::FOUND, [("Location", &redirect)]).into_response())
}

/// GET /library/{id}/preview/{*path} — preview a file (text or image).
pub async fn preview_file(
    user: WebUser,
    State(state): State<Arc<AppState>>,
    Path((repo_id, path)): Path<(String, String)>,
) -> Result<impl IntoResponse, AppError> {
    let db = state.db.as_ref();
    verify_repo_access(db, user.user_id, &repo_id).await?;

    // Get repo name for breadcrumb
    let repo_name = state
        .repos
        .repo
        .find_by_id(&repo_id)
        .await
        .map_err(|e| AppError::internal(format!("db error: {e}")))?
        .ok_or_else(|| AppError::NotFound("Repository not found".to_string()))?
        .name;

    let path = normalize_path(&path);
    let file_name = path.rsplit('/').next().unwrap_or("file").to_string();

    let raw_parent = parent_path_from(&path);
    let parent_path = raw_parent.trim_start_matches('/').to_string();

    // Check if this is an image file for special rendering.
    let is_image = file_name.ends_with(".png")
        || file_name.ends_with(".jpg")
        || file_name.ends_with(".jpeg")
        || file_name.ends_with(".gif")
        || file_name.ends_with(".webp")
        || file_name.ends_with(".bmp")
        || file_name.ends_with(".svg");

    // Check if this is a text/code file that can be previewed inline.
    let is_text = file_name.ends_with(".txt")
        || file_name.ends_with(".md")
        || file_name.ends_with(".rs")
        || file_name.ends_with(".py")
        || file_name.ends_with(".js")
        || file_name.ends_with(".ts")
        || file_name.ends_with(".html")
        || file_name.ends_with(".css")
        || file_name.ends_with(".go")
        || file_name.ends_with(".java")
        || file_name.ends_with(".c")
        || file_name.ends_with(".cpp")
        || file_name.ends_with(".h")
        || file_name.ends_with(".rb")
        || file_name.ends_with(".php")
        || file_name.ends_with(".sh")
        || file_name.ends_with(".toml")
        || file_name.ends_with(".json")
        || file_name.ends_with(".yaml")
        || file_name.ends_with(".yml")
        || file_name.ends_with(".csv")
        || file_name.ends_with(".xml")
        || file_name.ends_with(".sql")
        || file_name.ends_with(".conf")
        || file_name.ends_with(".ini")
        || file_name.ends_with(".log");

    if is_image {
        // Look up file size from the FS tree without downloading the full content.
        let size_display = get_file_size(db, &repo_id, &path)
            .await
            .map(format_size)
            .unwrap_or_else(|_| "?".to_string());

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
        };
        let html = tpl
            .render()
            .map_err(|e| AppError::internal(e.to_string()))?;
        return Ok(Html(html).into_response());
    }

    if is_text {
        // Text file preview
        let data = Downloader::download_file(db, &repo_id, &path, &state.block_store, None)
            .await
            .map_err(|e| AppError::Internal(format!("read failed: {e}")))?;

        let content = String::from_utf8_lossy(&data).to_string();
        let size_display = format_size(data.len() as i64);

        let tpl = PreviewTextTemplate {
            urls: crate::static_assets::template_urls(),
            user_email: user.email,
            is_admin: user.is_admin,
            repo_name,
            file_name,
            content,
            repo_id: repo_id.clone(),
            current_path: path,
            parent_path,
            size_display,
            active_page: "repos",
        };
        let html = tpl
            .render()
            .map_err(|e| AppError::internal(e.to_string()))?;
        return Ok(Html(html).into_response());
    }

    // Binary files (PDFs, archives, etc.) — redirect to download.
    let download_url = format!(
        "/library/{}/download/{}",
        repo_id,
        path.trim_start_matches('/')
    );
    Ok(axum::response::Redirect::to(&download_url).into_response())
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

/// GET /lib/{id}/file{*path} — Seahub-compatible file view.
///
/// Used by the Seafile desktop client's cloud file browser.
/// Supports query params:
///   ?raw=1 — returns raw file content
///   ?dl=1  — downloads the file
///   (none) — redirects to the preview page
pub async fn view_lib_file(
    user: WebUser,
    State(state): State<Arc<AppState>>,
    Path((repo_id, path)): Path<(String, String)>,
    Query(query): Query<ViewLibFileQuery>,
) -> Result<impl IntoResponse, AppError> {
    let db = state.db.as_ref();
    verify_repo_access(db, user.user_id, &repo_id).await?;

    let path = normalize_path(&path);
    let data = Downloader::download_file(db, &repo_id, &path, &state.block_store, None)
        .await
        .map_err(|e| AppError::Internal(format!("read failed: {e}")))?;

    let file_name = path.rsplit('/').next().unwrap_or("file");
    let content_type = mime_guess(file_name);

    if query.dl.as_deref() == Some("1") {
        // Download with Content-Disposition
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

    if query.raw.as_deref() == Some("1") {
        // Raw file content
        return Ok((StatusCode::OK, [(header::CONTENT_TYPE, content_type)], data).into_response());
    }

    // Default: redirect to the preview page
    let redirect = format!(
        "/library/{}/preview/{}",
        repo_id,
        path.trim_start_matches('/')
    );
    Ok((StatusCode::FOUND, [("Location", redirect.as_str())]).into_response())
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

/// Best-effort trash recording: reads the parent directory data to find the
/// entry's obj_id and type, then inserts into file_trash.  Silently ignores
/// all errors — trash recording must not break the delete operation.
async fn record_trash_best_effort(
    db: &DatabaseConnection,
    repo_id: &str,
    parent_fs_id: &str,
    full_path: &str,
    entry_name: &str,
    head_commit_id: &str,
    user_email: &str,
) {
    // Use .ok() to discard the non-Send Box<dyn Error> before any .await.
    let parent_data = crate::repo::read_fs_dir_data(db, repo_id, parent_fs_id)
        .await
        .ok();
    if let Some(pd) = parent_data
        && let Some(de) = pd.dirents.iter().find(|d| d.name == entry_name)
    {
        let ot = if de.mode & S_IFDIR != 0 {
            "dir"
        } else {
            "file"
        };
        let _ = crate::repo::trash::TrashService::add_to_trash(
            db,
            repo_id,
            full_path,
            ot,
            &de.id,
            &de.name,
            de.size,
            head_commit_id,
            user_email,
        )
        .await;
    }
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
