use axum::body::Body;
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use std::sync::Arc;

use base::common::DirEntry;

use crate::AppState;
use crate::service::fs::dir::{list_dir_from_fs_tree, list_dir_recursive_from_fs_tree};
use crate::webdav::auth::WebDavAuth;
use crate::webdav::util::{Depth, build_href, entry_metadata, http_date, join_path, parse_depth};
use crate::webdav::xml::{PropResponse, ResourceProps, build_multistatus};

/// PROPFIND handler. Returns a 207 Multi-Status document listing the target
/// and (depending on Depth) its descendants.
pub async fn propfind(
    state: Arc<AppState>,
    auth: WebDavAuth,
    path: String,
    headers: &HeaderMap,
    body: Body,
) -> Response {
    let depth = match parse_depth(headers) {
        Ok(d) => d,
        Err(code) => return code.into_response(),
    };
    let propname_only = body_requests_propname(body).await;

    let target_meta = match entry_metadata(&state.repos, &auth.repo_id, &path).await {
        Ok(Some(m)) => m,
        Ok(None) => return StatusCode::NOT_FOUND.into_response(),
        Err(_) => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    };
    let (target_is_dir, target_size, target_mtime) = target_meta;

    // Depth infinity on a non-collection is a client error.
    if !target_is_dir && matches!(depth, Depth::Infinity) {
        return StatusCode::BAD_REQUEST.into_response();
    }

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
            getcontentlength: if target_is_dir { 0 } else { target_size },
            getlastmodified: http_date(target_mtime),
        }),
    });

    if target_is_dir {
        match depth {
            Depth::Zero => {}
            Depth::One => {
                if let Ok((_, entries)) =
                    list_dir_from_fs_tree(&state.repos, &auth.repo_id, &path).await
                {
                    for e in entries {
                        let child = join_path(&path, &e.name);
                        push_entry(&mut responses, &auth.repo_id, &child, &e);
                    }
                }
            }
            Depth::Infinity => {
                if let Ok((_, entries)) =
                    list_dir_recursive_from_fs_tree(&state.repos, &auth.repo_id, &path).await
                {
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
            getcontentlength: if is_dir { 0 } else { e.size },
            getlastmodified: http_date(e.mtime),
        }),
    });
}

/// Detect whether the PROPFIND body requests property *names* only
/// (`<D:propname/>`). We return the same standard property set either way;
/// this only affects whether values are emitted.
async fn body_requests_propname(body: Body) -> bool {
    match axum::body::to_bytes(body, 64 * 1024).await {
        Ok(b) => String::from_utf8_lossy(&b)
            .to_lowercase()
            .contains("propname"),
        Err(_) => false,
    }
}
