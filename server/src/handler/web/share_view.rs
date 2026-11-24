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
use crate::fs::zip::{ZipLimits, acquire_zip_permit, collect_dir_entries, stream_zip};
use crate::i18n::I18n;
use crate::ui::files::BreadcrumbItem;
use crate::ui::format_size;
use base::common::FsFileData;
use base::error::AppError;
use infra::common::S_IFDIR;
use infra::entity::share_link;

/// Resolve a client-supplied sub-path against a share link's root path.
///
/// `safe_join_path` only clamps `..` to the **repo** root, so this additionally
/// requires the result to stay inside the shared subtree; every share handler
/// that turns client input into a repo path must go through here.
fn resolve_share_subpath(link: &share_link::Model, requested: &str) -> Result<String, AppError> {
    let resolved = base::sanitize::safe_join_path(&link.path, requested)
        .map_err(|e| AppError::BadRequest(format!("Invalid path: {e}")))?;

    let share_root =
        base::sanitize::safe_normalize_path(&link.path).unwrap_or_else(|_| "/".to_string());
    if share_root != "/"
        && resolved != share_root
        && !resolved.starts_with(&format!("{}/", share_root.trim_end_matches('/')))
    {
        return Err(AppError::BadRequest("Invalid path".into()));
    }

    Ok(resolved)
}

#[derive(Template)]
#[template(path = "web/share_view.html")]
struct ShareViewTemplate {
    pub t: &'static I18n,
    pub urls: &'static crate::static_assets::TemplateUrls,
    pub file_name: String,
    pub file_ext: String,
    pub file_size: String,
    pub has_password: bool,
    pub created_at_ts: i64,
    pub expires_at_ts: Option<i64>,
    pub download_url: String,
    pub description: Option<String>,
}

/// Rate-limit anonymous share-link downloads per client IP.
fn check_share_download_rate(
    state: &Arc<AppState>,
    addr: &SocketAddr,
    headers: &HeaderMap,
) -> Result<(), AppError> {
    let client_ip = crate::middleware::effective_client_ip(
        addr,
        headers,
        &state.config().server.trusted_proxies,
    );
    let key = format!("share_download:{client_ip}");
    if !state.auth_limiters.share_download.check_and_record(&key) {
        return Err(AppError::TooManyRequests);
    }
    Ok(())
}

/// Whether a password-protected share link is unlocked for this request.
///
/// The gate module owns this decision now; the file endpoint's raw error shape
/// is the only caller that passes `GateMode::Raw`.
async fn resolve_file_meta(
    repos: &crate::repository::Repositories,
    repo_id: &str,
    path: &str,
) -> Result<(FsFileData, Vec<String>), AppError> {
    Downloader::resolve_blocks(repos, repo_id, path)
        .await
        .map_err(|_| AppError::NotFound("File not found".into()))
}

/// GET /f/{token}/ — show HTML preview or download file.
pub async fn shared_file_view(
    State(state): State<Arc<AppState>>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    Path(token): Path<String>,
    headers: HeaderMap,
    Query(params): Query<HashMap<String, String>>,
) -> Result<Response, AppError> {
    let link = match super::link_gate::gate_get(
        &state,
        &headers,
        &token,
        super::link_gate::LinkKind::ShareFile,
        super::link_gate::GateMode::Page,
    )
    .await?
    {
        super::link_gate::Gate::Open { link, .. } => match link {
            super::link_gate::LinkModel::Share(link) => link,
            super::link_gate::LinkModel::Upload(_) => {
                return Err(AppError::internal("wrong link kind behind a share gate"));
            }
        },
        super::link_gate::Gate::Password(response) => return Ok(response),
    };

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

        let disposition = crate::fs::core::download::content_disposition(&filename, true);
        let range_header = headers.get(header::RANGE).and_then(|v| v.to_str().ok());
        let etag = crate::fs::core::download::blocks_etag(&block_ids);
        return Ok(crate::fs::core::download::file_download_response(
            crate::fs::core::download::FileDownloadParams {
                repo_id: link.repo_id.clone(),
                block_ids,
                block_store: state.block_store.clone(),
                enc_key: None,
                total_size: file_data.size.max(0) as u64,
                content_type: crate::ui::files::mime_guess(&filename),
                content_disposition: Some(disposition),
                range_header: range_header.map(|s| s.to_string()),
                etag: Some(etag),
                head_only: false,
                // The share URL is stable: revalidate so a new version shows up.
                cache: crate::fs::core::download::CachePolicy::PublicNoCache,
            },
            &headers,
        ));
    }

    let (_file_data, _block_ids) =
        resolve_file_meta(&state.repos, &link.repo_id, &link.path).await?;

    crate::service::sharing::share::increment_view_cnt(state.repos.share_link.clone(), link.id);

    let file_name = link
        .path
        .rsplit_once('/')
        .map(|(_, n)| n)
        .unwrap_or(&link.path)
        .to_string();
    // Uppercase, like the `.nf-doc-ext` tile in the file list.
    let file_ext = file_name
        .rsplit_once('.')
        .map(|(_, e)| e.to_uppercase())
        .unwrap_or_else(|| "?".to_string());
    let file_size = _file_data.size;

    // No password in the download URL: the browser carries the signed unlock
    // cookie instead, so it never lands in page links, history or logs.
    let download_url = format!("/f/{}/?dl=1", link.token);

    let tpl = ShareViewTemplate {
        t: I18n::from_headers(&headers, &state.config().ui.default_language),
        urls: crate::static_assets::template_urls(),
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

/// POST /f/{token}/ — validate the password and unlock the link with a cookie.
pub async fn shared_file_view_post(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    Path(token): Path<String>,
    axum::Form(form): axum::Form<HashMap<String, String>>,
) -> Result<Response, AppError> {
    super::link_gate::gate_post(
        &state,
        &headers,
        &addr,
        &token,
        super::link_gate::LinkKind::ShareFile,
        &form,
    )
    .await
}

#[derive(Template)]
#[template(path = "web/shared_dir_view.html")]
struct SharedDirViewTemplate {
    pub t: &'static I18n,
    pub urls: &'static crate::static_assets::TemplateUrls,
    pub token: String,
    pub dir_name: String,
    pub dir_path: String,
    pub parent_path: Option<String>,
    pub entries: Vec<DirEntryInfo>,
    pub breadcrumbs: Vec<BreadcrumbItem>,
    pub item_count: usize,
    pub has_password: bool,
    pub created_at_ts: i64,
    pub expires_at_ts: Option<i64>,
    pub download_url: String,
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
    let link = match super::link_gate::gate_get(
        &state,
        &headers,
        &token,
        super::link_gate::LinkKind::ShareDir,
        super::link_gate::GateMode::Page,
    )
    .await?
    {
        super::link_gate::Gate::Open { link, .. } => match link {
            super::link_gate::LinkModel::Share(link) => link,
            super::link_gate::LinkModel::Upload(_) => {
                return Err(AppError::internal("wrong link kind behind a share gate"));
            }
        },
        super::link_gate::Gate::Password(response) => return Ok(response),
    };

    if link.s_type != "d" {
        return Err(AppError::NotFound("Not a directory share link".into()));
    }

    let head = crate::fs::core::repo_head(&state.repos, &link.repo_id)
        .await
        .map_err(|e| match e {
            AppError::NotFound(_) => AppError::NotFound("Repo not found".into()),
            other => other,
        })?;
    if head.head_commit_id.is_none() {
        return Err(AppError::BadRequest("Repo has no commits".into()));
    }

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
        // Gate the expensive tree walk too, so a flood of anonymous link-token
        // requests can't bypass the global zip concurrency cap.
        let permit = acquire_zip_permit().await?;
        let files = collect_dir_entries(
            &state.repos,
            &link.repo_id,
            &head.root_id,
            &link.path,
            &dir_name,
            ZipLimits {
                max_entries: state.config().storage.max_zip_entries,
                max_bytes: state.config().storage.max_zip_bytes,
            },
        )
        .await?;

        crate::service::sharing::share::increment_view_cnt(state.repos.share_link.clone(), link.id);
        let stream = stream_zip(
            link.repo_id.clone(),
            state.block_store.clone(),
            files,
            None,
            permit,
        );

        let mut headers = HeaderMap::new();
        headers.insert(
            header::CONTENT_TYPE,
            HeaderValue::from_static("application/zip"),
        );
        headers.insert(
            HeaderName::from_static("content-disposition"),
            HeaderValue::from_str(&crate::fs::core::download::content_disposition(
                &format!("{dir_name}.zip"),
                true,
            ))
            .unwrap_or_else(|_| HeaderValue::from_static("attachment")),
        );
        return Ok((StatusCode::OK, headers, Body::from_stream(stream)).into_response());
    }

    crate::service::sharing::share::increment_view_cnt(state.repos.share_link.clone(), link.id);

    // Resolve the requested directory, clamped to the shared subtree (no `?p=../other-dir`).
    let sub_path = params.get("p").map(|s| s.as_str()).unwrap_or("/");
    let current_path = resolve_share_subpath(&link, sub_path)?;

    let dir_id = resolve_fs_id(&state.repos, &link.repo_id, &head.root_id, &current_path)
        .await
        .map_err(|_| AppError::NotFound("Directory not found".into()))?;

    let dir_data = read_fs_dir_data(&state.repos, &link.repo_id, &dir_id)
        .await
        .map_err(|_| AppError::NotFound("Directory not found".into()))?;

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
                .map(|(_, e)| e.to_uppercase())
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

    entries.sort_by(|a, b| {
        if a.is_dir != b.is_dir {
            b.is_dir.cmp(&a.is_dir)
        } else {
            a.name.cmp(&b.name)
        }
    });

    // Paginate on the sorted list so a huge folder doesn't render one multi-MB
    // page; `total` still drives the summary count.
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

    let t = I18n::from_headers(&headers, &state.config().ui.default_language);

    // `current_path` is repo-absolute, so the trail is relative to the share root.
    let share_root =
        base::sanitize::safe_normalize_path(&link.path).unwrap_or_else(|_| "/".to_string());
    let relative = current_path
        .strip_prefix(share_root.trim_end_matches('/'))
        .unwrap_or(&current_path);
    let breadcrumbs = crate::ui::files::breadcrumbs_for(relative);

    let dir_name = if current_path == "/" {
        // A whole-repo share has no folder name of its own; "/" reads as a path.
        t.tr("pub.root").to_string()
    } else {
        current_path
            .rsplit_once('/')
            .map(|(_, n)| n.to_string())
            .unwrap_or_else(|| current_path.clone())
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

    // No password in generated URLs: the signed unlock cookie carries the authorisation.
    let download_url = format!("/d/{}/?dl=1", link.token);
    let tpl = SharedDirViewTemplate {
        t,
        urls: crate::static_assets::template_urls(),
        token: link.token.clone(),
        dir_name,
        dir_path: sub_path.to_string(),
        parent_path,
        entries,
        breadcrumbs,
        item_count,
        has_password: link.password.is_some(),
        created_at_ts: link.created_at,
        expires_at_ts: link.expires_at,
        download_url,
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
///
/// The password arrives in the header or the signed unlock cookie, never the URL.
pub async fn shared_dir_file_view(
    State(state): State<Arc<AppState>>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    Path((token, file_path)): Path<(String, String)>,
    headers: HeaderMap,
) -> Result<Response, AppError> {
    let link = match super::link_gate::gate_get(
        &state,
        &headers,
        &token,
        super::link_gate::LinkKind::ShareDir,
        super::link_gate::GateMode::Raw,
    )
    .await?
    {
        super::link_gate::Gate::Open { link, .. } => match link {
            super::link_gate::LinkModel::Share(link) => link,
            super::link_gate::LinkModel::Upload(_) => {
                return Err(AppError::internal("wrong link kind behind a share gate"));
            }
        },
        super::link_gate::Gate::Password(_) => {
            return Err(AppError::internal("raw gate returned a page"));
        }
    };

    if link.s_type != "d" {
        return Err(AppError::NotFound("Not a directory share link".into()));
    }

    check_share_download_rate(&state, &addr, &headers)?;

    // Combine the share path with the requested path, clamped to the shared
    // subtree: a plain join would let `..` or an absolute path escape the share.
    let full_path = resolve_share_subpath(&link, &file_path)?;

    let (file_data, block_ids) = resolve_file_meta(&state.repos, &link.repo_id, &full_path).await?;

    crate::service::sharing::share::increment_view_cnt(state.repos.share_link.clone(), link.id);

    let filename = full_path
        .rsplit_once('/')
        .map(|(_, n)| n.to_string())
        .unwrap_or_else(|| full_path.clone());
    let disposition = crate::fs::core::download::content_disposition(&filename, true);
    let range_header = headers.get(header::RANGE).and_then(|v| v.to_str().ok());
    let etag = crate::fs::core::download::blocks_etag(&block_ids);
    Ok(crate::fs::core::download::file_download_response(
        crate::fs::core::download::FileDownloadParams {
            repo_id: link.repo_id.clone(),
            block_ids,
            block_store: state.block_store.clone(),
            enc_key: None,
            total_size: file_data.size.max(0) as u64,
            content_type: crate::ui::files::mime_guess(&filename),
            content_disposition: Some(disposition),
            range_header: range_header.map(|s| s.to_string()),
            etag: Some(etag),
            head_only: false,
            cache: crate::fs::core::download::CachePolicy::PublicNoCache,
        },
        &headers,
    ))
}

/// POST /d/{token}/ — validate the password and unlock the link with a cookie.
pub async fn shared_dir_view_post(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    Path(token): Path<String>,
    axum::Form(form): axum::Form<HashMap<String, String>>,
) -> Result<Response, AppError> {
    super::link_gate::gate_post(
        &state,
        &headers,
        &addr,
        &token,
        super::link_gate::LinkKind::ShareDir,
        &form,
    )
    .await
}
