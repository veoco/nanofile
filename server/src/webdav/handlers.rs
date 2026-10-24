use axum::body::Body;
use axum::extract::{Path, Request, State};
use axum::http::{HeaderMap, HeaderName, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use futures::TryStreamExt;
use std::collections::HashMap;
use std::sync::Arc;

use base::error::AppError;
use infra::common::util::{basename, parent_path_from};

use crate::AppState;
use crate::fs::core::FileOps;
use crate::webdav::auth::WebDavAuth;
use crate::webdav::util::{
    HeadContext, entry_metadata_with_head, join_path, normalize_webdav_path, parse_destination,
    parse_overwrite, resolve_head,
};
use crate::webdav::xml::{build_empty_multistatus, build_lock_body};

/// Entry point for `/dav/{repo_id}` and `/dav/{repo_id}/` (library root).
pub async fn dispatch_root(
    auth: WebDavAuth,
    State(state): State<Arc<AppState>>,
    Path(_repo_id): Path<String>,
    request: Request,
) -> Response {
    route_request(state, auth, "/".to_string(), request).await
}

/// Entry point for `/dav/{repo_id}/{*path}`.
pub async fn dispatch_path(
    auth: WebDavAuth,
    State(state): State<Arc<AppState>>,
    Path((_repo_id, raw_path)): Path<(String, String)>,
    request: Request,
) -> Response {
    let path = match normalize_webdav_path(&raw_path) {
        Ok(p) => p,
        Err(code) => return code.into_response(),
    };
    route_request(state, auth, path, request).await
}

async fn route_request(
    state: Arc<AppState>,
    auth: WebDavAuth,
    webdav_path: String,
    request: Request,
) -> Response {
    let method = request.method().clone();
    let (parts, body) = request.into_parts();
    let headers = parts.headers;

    // Resolve the repo + head commit once per request so no handler re-queries
    // them for each path it probes.
    let head = match resolve_head(&state.repos, &auth.repo_id).await {
        Ok(h) => h,
        Err(_) => return StatusCode::NOT_FOUND.into_response(),
    };

    match method.as_str() {
        "OPTIONS" => options_response(),
        "PROPFIND" => {
            crate::webdav::propfind::propfind(state, auth, webdav_path, &headers, body, &head).await
        }
        "GET" => get_handler(&state, &auth, &webdav_path, false, &head, &headers).await,
        "HEAD" => get_handler(&state, &auth, &webdav_path, true, &head, &headers).await,
        "PUT" => put_handler(&state, &auth, &webdav_path, &headers, body, &head).await,
        "MKCOL" => mkcol_handler(&state, &auth, &webdav_path, &head).await,
        "DELETE" => delete_handler(&state, &auth, &webdav_path, &head).await,
        "MOVE" => move_copy_handler(&state, &auth, &webdav_path, &headers, true, &head).await,
        "COPY" => move_copy_handler(&state, &auth, &webdav_path, &headers, false, &head).await,
        "PROPPATCH" => proppatch_response(),
        "LOCK" => lock_response(),
        "UNLOCK" => unlock_response(),
        _ => StatusCode::METHOD_NOT_ALLOWED.into_response(),
    }
}

async fn get_handler(
    state: &Arc<AppState>,
    auth: &WebDavAuth,
    path: &str,
    is_head: bool,
    head: &HeadContext,
    headers: &HeaderMap,
) -> Response {
    // Resolve fs_id + metadata in one tree walk, then read the fs_object directly.
    let fs_id = match crate::fs::core::resolve_file_entry_with_head(
        &state.repos,
        &auth.repo_id,
        &head.root_id,
        path,
    )
    .await
    {
        Ok(Some((_, true, _, _))) => return StatusCode::METHOD_NOT_ALLOWED.into_response(),
        Ok(Some((fs_id, false, _, _))) => fs_id,
        Ok(None) => return StatusCode::NOT_FOUND.into_response(),
        Err(_) => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    };

    let file_data =
        match crate::fs::core::FileOps::read_file_fs_object(&state.repos, &auth.repo_id, &fs_id)
            .await
        {
            Ok(d) => d,
            Err(_) => return StatusCode::NOT_FOUND.into_response(),
        };
    let block_ids = file_data.block_ids;
    // The response body comes from the block list, so its size is authoritative; the tree's
    // `size` is only what the entry recorded.
    let total_size = file_data.size.max(0) as u64;

    let filename = path.rsplit_once('/').map(|(_, n)| n).unwrap_or("download");

    // A strong validator, so a client can revalidate instead of re-downloading, and `Range`
    // works for media players and resumable clients. Both come from the one download builder the
    // web endpoints use, so `ETag`/`Accept-Ranges`/`Content-Range` cannot drift from theirs.
    // Encrypted libraries never reach here: `WebDavAuth` refuses them, so no key is passed.
    let etag = crate::fs::core::download::blocks_etag(&block_ids);
    let range_header = headers.get(header::RANGE).and_then(|v| v.to_str().ok());
    if range_header.is_none() && crate::fs::core::download::if_none_match(headers, &etag) {
        let mut resp = StatusCode::NOT_MODIFIED.into_response();
        if let Ok(value) = HeaderValue::from_str(&etag) {
            resp.headers_mut().insert(header::ETAG, value);
        }
        return resp;
    }

    crate::fs::core::download::file_download_response(
        crate::fs::core::download::FileDownloadParams {
            repo_id: auth.repo_id.clone(),
            block_ids,
            block_store: state.block_store.clone(),
            enc_key: None,
            total_size,
            content_type: crate::ui::files::mime_guess(filename),
            content_disposition: None,
            range_header: range_header.map(str::to_string),
            etag: Some(etag),
            head_only: is_head,
        },
    )
}

async fn put_handler(
    state: &Arc<AppState>,
    auth: &WebDavAuth,
    path: &str,
    headers: &HeaderMap,
    body: Body,
    head: &HeadContext,
) -> Response {
    if auth.permission != "rw" {
        return StatusCode::FORBIDDEN.into_response();
    }
    if path == "/" {
        return StatusCode::METHOD_NOT_ALLOWED.into_response();
    }
    let name = match path.rsplit_once('/') {
        Some((_, n)) if !n.is_empty() => n,
        _ => return StatusCode::BAD_REQUEST.into_response(),
    };
    if base::sanitize::validate_filename(name).is_err() {
        return StatusCode::BAD_REQUEST.into_response();
    }

    let parent = parent_path_from(path);
    // The parent must exist (RFC 4918: 409 Conflict otherwise).
    match entry_metadata_with_head(&state.repos, &auth.repo_id, head, parent).await {
        Ok(Some((true, _, _))) => {}
        _ => return StatusCode::CONFLICT.into_response(),
    }

    let existed = matches!(
        entry_metadata_with_head(&state.repos, &auth.repo_id, head, path).await,
        Ok(Some(_))
    );

    // Stream into CDC-chunked block writes, so a large PUT is never buffered whole.
    let content_length = headers
        .get(header::CONTENT_LENGTH)
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.parse::<usize>().ok())
        .unwrap_or(0);

    let stream = body
        .into_data_stream()
        .map_err(|e| std::io::Error::other(format!("WebDAV PUT body read failed: {e}")));

    let (block_ids, total_size, new_block_ids) = match FileOps::write_stream_blocks(
        &state.block_store,
        &auth.repo_id,
        content_length,
        stream,
        None,
    )
    .await
    {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!("WebDAV PUT block write failed for {path}: {e}");
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
    };

    match state
        .file_service()
        .upload_file_committed_stream(
            &auth.repo_id,
            parent,
            name,
            block_ids,
            total_size,
            &auth.email,
            Some(auth.user_id),
            false,
            // WebDAV PUT overwrites an existing resource, but a new one must
            // still be committed as a create, so pass `existed`.
            existed,
            new_block_ids,
        )
        .await
    {
        Ok(_) => {
            if existed {
                StatusCode::NO_CONTENT.into_response()
            } else {
                StatusCode::CREATED.into_response()
            }
        }
        Err(e) => {
            tracing::warn!("WebDAV PUT failed for {path}: {e}");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

async fn mkcol_handler(
    state: &Arc<AppState>,
    auth: &WebDavAuth,
    path: &str,
    head: &HeadContext,
) -> Response {
    if auth.permission != "rw" {
        return StatusCode::FORBIDDEN.into_response();
    }
    if path == "/" {
        return StatusCode::METHOD_NOT_ALLOWED.into_response();
    }
    let name = match path.rsplit_once('/') {
        Some((_, n)) if !n.is_empty() => n,
        _ => return StatusCode::BAD_REQUEST.into_response(),
    };
    if base::sanitize::validate_filename(name).is_err() {
        return StatusCode::BAD_REQUEST.into_response();
    }
    // Target must not already exist (405).
    if matches!(
        entry_metadata_with_head(&state.repos, &auth.repo_id, head, path).await,
        Ok(Some(_))
    ) {
        return StatusCode::METHOD_NOT_ALLOWED.into_response();
    }
    let parent = parent_path_from(path);
    match entry_metadata_with_head(&state.repos, &auth.repo_id, head, parent).await {
        Ok(Some((true, _, _))) => {}
        _ => return StatusCode::CONFLICT.into_response(),
    }
    match state
        .dir_service()
        .create_dir(&auth.repo_id, path, &auth.email, auth.user_id)
        .await
    {
        Ok(()) => StatusCode::CREATED.into_response(),
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

async fn delete_handler(
    state: &Arc<AppState>,
    auth: &WebDavAuth,
    path: &str,
    head: &HeadContext,
) -> Response {
    if auth.permission != "rw" {
        return StatusCode::FORBIDDEN.into_response();
    }
    if path == "/" {
        return StatusCode::METHOD_NOT_ALLOWED.into_response();
    }
    let meta = match entry_metadata_with_head(&state.repos, &auth.repo_id, head, path).await {
        Ok(m) => m,
        Err(_) => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    };
    let Some((is_dir, _, _)) = meta else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let obj = if is_dir { "dir" } else { "file" };
    match state
        .dir_service()
        .delete_dirent(&auth.repo_id, obj, path, &auth.email, auth.user_id)
        .await
    {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

/// Memoized `entry_metadata_with_head`; MOVE/COPY probes src and dst twice.
async fn cached_entry_metadata(
    cache: &mut HashMap<String, Option<(bool, i64, i64)>>,
    repos: &crate::repository::Repositories,
    repo_id: &str,
    head: &HeadContext,
    path: &str,
) -> Option<Option<(bool, i64, i64)>> {
    if let Some(v) = cache.get(path) {
        return Some(*v);
    }
    let v = entry_metadata_with_head(repos, repo_id, head, path)
        .await
        .ok()?;
    cache.insert(path.to_string(), v);
    Some(v)
}

async fn move_copy_handler(
    state: &Arc<AppState>,
    auth: &WebDavAuth,
    src_path: &str,
    headers: &HeaderMap,
    is_move: bool,
    head: &HeadContext,
) -> Response {
    // Per-path metadata cache: repeated src/dst probes skip the tree walk.
    let mut meta_cache: HashMap<String, Option<(bool, i64, i64)>> = HashMap::new();

    if auth.permission != "rw" {
        return StatusCode::FORBIDDEN.into_response();
    }
    if src_path == "/" {
        return StatusCode::FORBIDDEN.into_response();
    }
    let dest_str = match headers.get("destination").and_then(|v| v.to_str().ok()) {
        Some(d) => d,
        None => return StatusCode::BAD_REQUEST.into_response(),
    };
    let (dst_repo, dst_path) = match parse_destination(dest_str) {
        Ok(v) => v,
        Err(code) => return code.into_response(),
    };
    if dst_repo != auth.repo_id {
        return StatusCode::FORBIDDEN.into_response();
    }
    if dst_path == "/" || dst_path == src_path {
        return StatusCode::FORBIDDEN.into_response();
    }
    // Moving/copying a directory into its own subtree is invalid.
    if let Some(Some((true, _, _))) =
        cached_entry_metadata(&mut meta_cache, &state.repos, &auth.repo_id, head, src_path).await
    {
        let prefix = format!("{}/", src_path);
        if dst_path.starts_with(&prefix) {
            return StatusCode::FORBIDDEN.into_response();
        }
    }

    let overwrite = parse_overwrite(headers);

    // Destination parent must exist.
    let dst_parent = parent_path_from(&dst_path);
    match cached_entry_metadata(
        &mut meta_cache,
        &state.repos,
        &auth.repo_id,
        head,
        dst_parent,
    )
    .await
    {
        Some(Some((true, _, _))) => {}
        _ => return StatusCode::CONFLICT.into_response(),
    }

    let dst_meta = cached_entry_metadata(
        &mut meta_cache,
        &state.repos,
        &auth.repo_id,
        head,
        &dst_path,
    )
    .await;
    let dst_existed = matches!(dst_meta, Some(Some(_)));
    if dst_existed {
        if !overwrite {
            return StatusCode::PRECONDITION_FAILED.into_response();
        }
        // Remove the target first, else batch_copy/move auto-renames it (Overwrite).
        if let Some(Some((is_dir, _, _))) = dst_meta {
            let obj = if is_dir { "dir" } else { "file" };
            if state
                .dir_service()
                .delete_dirent(&auth.repo_id, obj, &dst_path, &auth.email, auth.user_id)
                .await
                .is_err()
            {
                return StatusCode::INTERNAL_SERVER_ERROR.into_response();
            }
        }
    }

    let result = if is_move {
        do_move(state, auth, src_path, &dst_path, head).await
    } else {
        do_copy(state, auth, src_path, &dst_path, head).await
    };
    match result {
        Ok(()) => {
            if dst_existed {
                StatusCode::NO_CONTENT.into_response()
            } else {
                StatusCode::CREATED.into_response()
            }
        }
        Err(e) => {
            tracing::warn!("WebDAV MOVE/COPY failed: {e}");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

async fn do_move(
    state: &Arc<AppState>,
    auth: &WebDavAuth,
    src: &str,
    dst: &str,
    head: &HeadContext,
) -> Result<(), AppError> {
    let src_parent = parent_path_from(src);
    let src_name = basename(src);
    let dst_parent = parent_path_from(dst);
    let dst_name = basename(dst);

    let is_dir = entry_metadata_with_head(&state.repos, &auth.repo_id, head, src)
        .await?
        .map(|m| m.0)
        .unwrap_or(false);

    if src_parent == dst_parent {
        if is_dir {
            state
                .dir_service()
                .rename_dir_entry(&auth.repo_id, src, dst_name, &auth.email, auth.user_id)
                .await?;
        } else {
            state
                .file_service()
                .rename_file(&auth.repo_id, src, dst_name, &auth.email, auth.user_id)
                .await?;
        }
        return Ok(());
    }

    // Move to the destination parent, keeping the name, then rename if needed.
    if is_dir {
        state
            .dir_service()
            .move_dir(&auth.repo_id, src, dst_parent, &auth.email, auth.user_id)
            .await?;
    } else {
        state
            .file_service()
            .move_file(&auth.repo_id, src, dst_parent, &auth.email, auth.user_id)
            .await?;
    }
    if dst_name != src_name {
        let moved_path = join_path(dst_parent, src_name);
        if is_dir {
            state
                .dir_service()
                .rename_dir_entry(
                    &auth.repo_id,
                    &moved_path,
                    dst_name,
                    &auth.email,
                    auth.user_id,
                )
                .await?;
        } else {
            state
                .file_service()
                .rename_file(
                    &auth.repo_id,
                    &moved_path,
                    dst_name,
                    &auth.email,
                    auth.user_id,
                )
                .await?;
        }
    }
    Ok(())
}

async fn do_copy(
    state: &Arc<AppState>,
    auth: &WebDavAuth,
    src: &str,
    dst: &str,
    head: &HeadContext,
) -> Result<(), AppError> {
    let src_parent = parent_path_from(src);
    let src_name = basename(src);
    let dst_parent = parent_path_from(dst);
    let dst_name = basename(dst);

    let names = vec![src_name.to_string()];
    state
        .fileops_service()
        .batch_copy(
            &auth.repo_id,
            src_parent,
            dst_parent,
            &names,
            &auth.email,
            auth.user_id,
        )
        .await?;

    if dst_name != src_name {
        let copied_path = join_path(dst_parent, src_name);
        let is_dir = entry_metadata_with_head(&state.repos, &auth.repo_id, head, &copied_path)
            .await?
            .map(|m| m.0)
            .unwrap_or(false);
        if is_dir {
            state
                .dir_service()
                .rename_dir_entry(
                    &auth.repo_id,
                    &copied_path,
                    dst_name,
                    &auth.email,
                    auth.user_id,
                )
                .await?;
        } else {
            state
                .file_service()
                .rename_file(
                    &auth.repo_id,
                    &copied_path,
                    dst_name,
                    &auth.email,
                    auth.user_id,
                )
                .await?;
        }
    }
    Ok(())
}

fn options_response() -> Response {
    let mut resp = StatusCode::OK.into_response();
    let headers = resp.headers_mut();
    headers.insert(
        HeaderName::from_static("dav"),
        HeaderValue::from_static("1, 2"),
    );
    headers.insert(
        header::ALLOW,
        HeaderValue::from_static(
            "OPTIONS, GET, HEAD, PUT, MKCOL, DELETE, PROPFIND, PROPPATCH, MOVE, COPY, LOCK, UNLOCK",
        ),
    );
    headers.insert(
        HeaderName::from_static("ms-author-via"),
        HeaderValue::from_static("DAV"),
    );
    resp
}

fn proppatch_response() -> Response {
    let xml = build_empty_multistatus();
    (
        StatusCode::MULTI_STATUS,
        [(header::CONTENT_TYPE, "application/xml; charset=\"utf-8\"")],
        xml,
    )
        .into_response()
}

/// No-op LOCK. Always succeeds with a fresh token so Windows/macOS clients
/// can complete their lock handshakes; locks are never enforced.
fn lock_response() -> Response {
    let token = format!("opaquelocktoken:{}", uuid::Uuid::new_v4());
    let body = build_lock_body(&token);
    let mut resp = (
        StatusCode::OK,
        [(header::CONTENT_TYPE, "application/xml; charset=\"utf-8\"")],
        body,
    )
        .into_response();
    if let Ok(v) = HeaderValue::from_str(&format!("<{token}>")) {
        resp.headers_mut()
            .insert(HeaderName::from_static("lock-token"), v);
    }
    resp
}

fn unlock_response() -> Response {
    StatusCode::NO_CONTENT.into_response()
}
