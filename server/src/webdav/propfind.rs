use axum::body::Body;
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use std::sync::Arc;

use base::common::DirEntry;

use crate::AppState;
use crate::service::fs::dir::{list_dir_from_root, list_dir_recursive_from_root};
use crate::webdav::auth::WebDavAuth;
use crate::webdav::util::{
    Depth, HeadContext, build_href, entry_metadata_with_head, http_date, join_path, parse_depth,
};
use crate::webdav::xml::{
    PropResponse, ResourceProps, build_finite_depth_error, build_multistatus,
};

/// PROPFIND handler: a 207 Multi-Status listing the target and its descendants.
pub async fn propfind(
    state: Arc<AppState>,
    auth: WebDavAuth,
    path: String,
    headers: &HeaderMap,
    body: Body,
    head: &HeadContext,
) -> Response {
    let depth = match parse_depth(headers) {
        Ok(d) => d,
        Err(code) => return code.into_response(),
    };
    let propname_only = body_requests_propname(body).await;

    let target_meta = match entry_metadata_with_head(&state.repos, &auth.repo_id, head, &path).await
    {
        Ok(Some(m)) => m,
        Ok(None) => return StatusCode::NOT_FOUND.into_response(),
        Err(_) => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    };
    let target_is_dir = target_meta.is_dir;

    // Depth infinity on a non-collection is a client error.
    if !target_is_dir && matches!(depth, Depth::Infinity) {
        return StatusCode::BAD_REQUEST.into_response();
    }

    // The multistatus is built in memory and an absent `Depth` means infinity, so
    // over the cap the client is told to retry shallower, not given a truncated list.
    // RFC 4918 §9.1 spells exactly that refusal: 403 with a
    // `<D:propfind-finite-depth/>` precondition (wsgidav/seafdav answers the same
    // when it disables infinite depth; 507 would claim the storage is full).
    let limit = state.config().server.max_propfind_entries;
    let exceeds_limit = |count: usize| limit > 0 && count > limit;

    let mut responses = Vec::new();

    let displayname = if path == "/" {
        state
            .repos
            .repo
            .find_by_id(&auth.repo_id)
            .await
            .ok()
            .flatten()
            .map(|r| r.name)
            .unwrap_or_default()
    } else {
        path.rsplit_once('/')
            .map(|(_, n)| n)
            .unwrap_or("")
            .to_string()
    };

    let mut target_href = build_href(&auth.repo_id, &path);
    if target_is_dir && !target_href.ends_with('/') {
        target_href.push('/');
    }
    responses.push(PropResponse {
        href: target_href,
        props: Some(ResourceProps {
            is_collection: target_is_dir,
            displayname,
            // Collections carry no size (wsgidav omits `getcontentlength` for them).
            getcontentlength: (!target_is_dir).then_some(target_meta.size),
            getlastmodified: http_date(target_meta.mtime),
            etag: file_etag(!target_is_dir, &target_meta.fs_id),
        }),
    });

    if target_is_dir {
        match depth {
            Depth::Zero => {}
            Depth::One => {
                if let Ok((_, entries)) =
                    list_dir_from_root(&state.repos, &auth.repo_id, &head.root_id, &path).await
                {
                    if exceeds_limit(entries.len() + responses.len()) {
                        return finite_depth_refusal();
                    }
                    for e in entries {
                        let child = join_path(&path, &e.name);
                        push_entry(&mut responses, &auth.repo_id, &child, &e);
                    }
                }
            }
            Depth::Infinity => {
                // The walk stops at the cap instead of building the whole tree and only then
                // refusing it, so a refused request costs about as much as an accepted one.
                let budget = if limit > 0 {
                    Some(limit.saturating_sub(responses.len()))
                } else {
                    None
                };
                if let Ok((_, entries, truncated)) = list_dir_recursive_from_root(
                    &state.repos,
                    &auth.repo_id,
                    &head.root_id,
                    &path,
                    false,
                    budget,
                )
                .await
                {
                    if truncated || exceeds_limit(entries.len() + responses.len()) {
                        return finite_depth_refusal();
                    }
                    for e in entries {
                        let parent = e.parent_dir.as_deref().unwrap_or(&path);
                        let child = join_path(parent, &e.name);
                        push_entry(&mut responses, &auth.repo_id, &child, &e);
                    }
                }
            }
        }
    }

    let xml = build_multistatus(&responses, propname_only);
    let mut resp_headers = HeaderMap::new();
    resp_headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/xml; charset=\"utf-8\""),
    );
    resp_headers.insert(
        header::CONTENT_LENGTH,
        HeaderValue::from_str(&xml.len().to_string()).unwrap(),
    );
    (StatusCode::MULTI_STATUS, resp_headers, xml).into_response()
}

fn push_entry(responses: &mut Vec<PropResponse>, repo_id: &str, full_path: &str, e: &DirEntry) {
    let is_dir = e.entry_type == "dir";
    let mut href = build_href(repo_id, full_path);
    if is_dir && !href.ends_with('/') {
        href.push('/');
    }
    responses.push(PropResponse {
        href,
        props: Some(ResourceProps {
            is_collection: is_dir,
            displayname: e.name.clone(),
            getcontentlength: (!is_dir).then_some(e.size),
            getlastmodified: http_date(e.mtime),
            etag: file_etag(!is_dir, &e.id),
        }),
    });
}

/// Detect whether the PROPFIND body requests property *names* only
/// (`<D:propname/>`); the same property set is returned either way.
async fn body_requests_propname(body: Body) -> bool {
    match axum::body::to_bytes(body, 64 * 1024).await {
        Ok(b) => String::from_utf8_lossy(&b)
            .to_lowercase()
            .contains("propname"),
        Err(_) => false,
    }
}

/// The `getetag` property value of a file, quoted exactly like the `ETag` header a
/// `GET` returns for it (`seafdav` reports the FS object id in both places, so a
/// client that caches on one value can revalidate against the other). Directories
/// have no `GET`, so they carry no validator.
fn file_etag(emit: bool, fs_id: &str) -> Option<String> {
    emit.then(|| format!("\"{fs_id}\""))
}

/// Response to a `Depth: infinity` listing that would exceed `max_propfind_entries`.
fn finite_depth_refusal() -> Response {
    let mut headers = HeaderMap::new();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/xml; charset=\"utf-8\""),
    );
    (StatusCode::FORBIDDEN, headers, build_finite_depth_error()).into_response()
}
