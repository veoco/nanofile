use axum::body::Body;
use axum::extract::{Path, Request, State};
use axum::http::{HeaderMap, HeaderName, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use std::sync::Arc;

use base::error::AppError;
use infra::common::util::{basename, parent_path_from};

use crate::AppState;
use crate::service::fs::file::UploadedFile;
use crate::webdav::auth::WebDavAuth;
use crate::webdav::util::{
    entry_metadata, join_path, normalize_webdav_path, parse_destination, parse_overwrite,
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

/// Route a WebDAV request to the appropriate method handler.
async fn route_request(
    state: Arc<AppState>,
    auth: WebDavAuth,
    webdav_path: String,
    request: Request,
) -> Response {
    let method = request.method().clone();
    let (parts, body) = request.into_parts();
    let headers = parts.headers;
    match method.as_str() {
        "OPTIONS" => options_response(),
        "PROPFIND" => {
            crate::webdav::propfind::propfind(state, auth, webdav_path, &headers, body).await
        }
        "GET" => get_handler(&state, &auth, &webdav_path, false).await,
        "HEAD" => get_handler(&state, &auth, &webdav_path, true).await,
        "PUT" => put_handler(&state, &auth, &webdav_path, body).await,
        "MKCOL" => mkcol_handler(&state, &auth, &webdav_path).await,
        "DELETE" => delete_handler(&state, &auth, &webdav_path).await,
        "MOVE" => move_copy_handler(&state, &auth, &webdav_path, &headers, true).await,
        "COPY" => move_copy_handler(&state, &auth, &webdav_path, &headers, false).await,
        "PROPPATCH" => proppatch_response(),
        "LOCK" => lock_response(),
        "UNLOCK" => unlock_response(),
        _ => StatusCode::METHOD_NOT_ALLOWED.into_response(),
    }
}

// ── Read methods ─────────────────────────────────────────────────────────

async fn get_handler(
    state: &Arc<AppState>,
    auth: &WebDavAuth,
    path: &str,
    is_head: bool,
) -> Response {
    match entry_metadata(&state.repos, &auth.repo_id, path).await {
        Ok(Some((true, _, _))) => return StatusCode::METHOD_NOT_ALLOWED.into_response(),
        Ok(Some((false, _, _))) => {}
        Ok(None) => return StatusCode::NOT_FOUND.into_response(),
        Err(_) => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }

    let (file_data, block_ids) = match crate::fs::core::download::Downloader::resolve_blocks(
        &state.repos,
        &auth.repo_id,
        path,
    )
    .await
    {
        Ok(v) => v,
        Err(_) => return StatusCode::NOT_FOUND.into_response(),
    };

    let filename = path.rsplit_once('/').map(|(_, n)| n).unwrap_or("download");
    let mime = crate::ui::files::mime_guess(filename);

    let mut resp_headers = HeaderMap::new();
    resp_headers.insert(header::CONTENT_TYPE, HeaderValue::from_static(mime));
    resp_headers.insert(
        header::CONTENT_LENGTH,
        HeaderValue::from_str(&file_data.size.to_string()).unwrap(),
    );

    if is_head {
        return (StatusCode::OK, resp_headers).into_response();
    }
    let stream =
        crate::fs::core::download::stream_blocks(block_ids, state.block_store.clone(), None);
    (StatusCode::OK, resp_headers, Body::from_stream(stream)).into_response()
}

// ── Write methods ────────────────────────────────────────────────────────

async fn put_handler(state: &Arc<AppState>, auth: &WebDavAuth, path: &str, body: Body) -> Response {
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
    match entry_metadata(&state.repos, &auth.repo_id, parent).await {
        Ok(Some((true, _, _))) => {}
        _ => return StatusCode::CONFLICT.into_response(),
    }

    let existed = matches!(
        entry_metadata(&state.repos, &auth.repo_id, path).await,
        Ok(Some(_))
    );

    let bytes = match axum::body::to_bytes(body, usize::MAX).await {
        Ok(b) => b,
        Err(_) => return StatusCode::BAD_REQUEST.into_response(),
    };

    let upload = UploadedFile {
        file_name: name.to_string(),
        file_data: bytes.to_vec(),
        parent_dir: parent.to_string(),
        replace: true,
    };
    match state
        .file_service()
        .upload_file(&auth.repo_id, upload, &auth.email, auth.user_id)
        .await
    {
        Ok(()) => {
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

async fn mkcol_handler(state: &Arc<AppState>, auth: &WebDavAuth, path: &str) -> Response {
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
        entry_metadata(&state.repos, &auth.repo_id, path).await,
        Ok(Some(_))
    ) {
        return StatusCode::METHOD_NOT_ALLOWED.into_response();
    }
    let parent = parent_path_from(path);
    match entry_metadata(&state.repos, &auth.repo_id, parent).await {
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

async fn delete_handler(state: &Arc<AppState>, auth: &WebDavAuth, path: &str) -> Response {
    if auth.permission != "rw" {
        return StatusCode::FORBIDDEN.into_response();
    }
    if path == "/" {
        return StatusCode::METHOD_NOT_ALLOWED.into_response();
    }
    let meta = match entry_metadata(&state.repos, &auth.repo_id, path).await {
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

async fn move_copy_handler(
    state: &Arc<AppState>,
    auth: &WebDavAuth,
    src_path: &str,
    headers: &HeaderMap,
    is_move: bool,
) -> Response {
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
    if let Ok(Some((true, _, _))) = entry_metadata(&state.repos, &auth.repo_id, src_path).await {
        let prefix = format!("{}/", src_path);
        if dst_path.starts_with(&prefix) {
            return StatusCode::FORBIDDEN.into_response();
        }
    }

    let overwrite = parse_overwrite(headers);

    // Destination parent must exist.
    let dst_parent = parent_path_from(&dst_path);
    match entry_metadata(&state.repos, &auth.repo_id, dst_parent).await {
        Ok(Some((true, _, _))) => {}
        _ => return StatusCode::CONFLICT.into_response(),
    }

    let dst_existed = matches!(
        entry_metadata(&state.repos, &auth.repo_id, &dst_path).await,
        Ok(Some(_))
    );
    if dst_existed {
        if !overwrite {
            return StatusCode::PRECONDITION_FAILED.into_response();
        }
        // Remove the target first so batch_copy/batch_move don't auto-rename
        // the entry (WebDAV Overwrite semantics).
        if let Ok(Some((is_dir, _, _))) =
            entry_metadata(&state.repos, &auth.repo_id, &dst_path).await
        {
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
        do_move(state, auth, src_path, &dst_path).await
    } else {
        do_copy(state, auth, src_path, &dst_path).await
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
) -> Result<(), AppError> {
    let src_parent = parent_path_from(src);
    let src_name = basename(src);
    let dst_parent = parent_path_from(dst);
    let dst_name = basename(dst);

    let is_dir = entry_metadata(&state.repos, &auth.repo_id, src)
        .await?
        .map(|m| m.0)
        .unwrap_or(false);

    if src_parent == dst_parent {
        // Pure rename within the same directory.
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

    // Move to the destination parent, keeping the name.
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
        let is_dir = entry_metadata(&state.repos, &auth.repo_id, &copied_path)
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

// ── Auxiliary methods ────────────────────────────────────────────────────

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
