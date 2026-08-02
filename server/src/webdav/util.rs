//! Shared helpers for the WebDAV handlers.

use axum::http::{HeaderMap, StatusCode};
use percent_encoding::{AsciiSet, CONTROLS};

use base::error::AppError;
use infra::common::util::parent_path_from;
use infra::serialization::S_IFDIR;

use crate::fs::core::{read_fs_dir_data, resolve_fs_id};
use crate::repository::Repositories;

/// Characters percent-encoded when building `<D:href>` values. Keeps `/`
/// intact so the path remains parseable.
pub const HREF_ENCODE_SET: &AsciiSet = &CONTROLS
    .add(b' ')
    .add(b'"')
    .add(b'#')
    .add(b'%')
    .add(b'<')
    .add(b'>')
    .add(b'?')
    .add(b'`')
    .add(b'{')
    .add(b'}')
    .add(b'\\')
    .add(b'^')
    .add(b'|')
    .add(b'[')
    .add(b']');

/// The `Depth` header value for PROPFIND / MOVE / COPY.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Depth {
    Zero,
    One,
    Infinity,
}

/// Parse the `Depth` header. Defaults to `infinity` per RFC 4918; invalid
/// values produce a 400 status.
pub fn parse_depth(headers: &HeaderMap) -> Result<Depth, StatusCode> {
    match headers.get("depth").and_then(|v| v.to_str().ok()) {
        None => Ok(Depth::Infinity),
        Some("0") => Ok(Depth::Zero),
        Some("1") => Ok(Depth::One),
        Some("infinity") => Ok(Depth::Infinity),
        Some(_) => Err(StatusCode::BAD_REQUEST),
    }
}

/// Parse the `Overwrite` header. Defaults to `true` (RFC 4918).
pub fn parse_overwrite(headers: &HeaderMap) -> bool {
    !matches!(
        headers.get("overwrite").and_then(|v| v.to_str().ok()),
        Some("F")
    )
}

/// Normalize a raw `{*path}` capture into a canonical in-repo path.
pub fn normalize_webdav_path(raw: &str) -> Result<String, StatusCode> {
    let p = format!("/{raw}");
    base::sanitize::safe_normalize_path(&p).map_err(|_| StatusCode::BAD_REQUEST)
}

/// Parse a `Destination` header (an absolute URL or a path) into
/// `(repo_id, normalized_in_repo_path)`.
pub fn parse_destination(dest: &str) -> Result<(String, String), StatusCode> {
    let after_scheme = match dest.split_once("://") {
        Some((_, rest)) => rest,
        None => dest,
    };
    let path_part = if after_scheme.starts_with('/') {
        after_scheme.to_string()
    } else {
        match after_scheme.split_once('/') {
            Some((_authority, path)) => format!("/{path}"),
            None => return Err(StatusCode::BAD_REQUEST),
        }
    };
    let path = path_part.split(['?', '#']).next().unwrap_or("/");
    let decoded = percent_encoding::percent_decode_str(path)
        .decode_utf8_lossy()
        .to_string();
    let rest = decoded
        .strip_prefix("/dav/")
        .ok_or(StatusCode::BAD_REQUEST)?;
    let (repo_id, inner) = match rest.split_once('/') {
        Some((r, p)) => (r.to_string(), p.to_string()),
        None => (rest.to_string(), String::new()),
    };
    if repo_id.is_empty() {
        return Err(StatusCode::BAD_REQUEST);
    }
    let inner_path = if inner.is_empty() {
        "/".to_string()
    } else {
        format!("/{inner}")
    };
    let normalized =
        base::sanitize::safe_normalize_path(&inner_path).map_err(|_| StatusCode::BAD_REQUEST)?;
    Ok((repo_id, normalized))
}

/// Resolve a path's metadata from the FS tree.
///
/// Returns `Ok(None)` when the path does not exist, and `Ok(Some((is_dir,
/// size, mtime)))` otherwise.
pub async fn entry_metadata(
    repos: &Repositories,
    repo_id: &str,
    path: &str,
) -> Result<Option<(bool, i64, i64)>, AppError> {
    if path == "/" || path.is_empty() {
        let repo = repos
            .repo
            .find_by_id(repo_id)
            .await?
            .ok_or_else(|| AppError::NotFound("repo not found".into()))?;
        return Ok(Some((true, repo.size, repo.updated_at)));
    }

    let repo_record = repos
        .repo
        .find_by_id(repo_id)
        .await?
        .ok_or_else(|| AppError::NotFound("repo not found".into()))?;
    let Some(head_commit_id) = repo_record.head_commit_id else {
        return Ok(None);
    };
    let head = repos
        .commit
        .find_by_repo_and_commit_id(repo_id, &head_commit_id)
        .await?
        .ok_or_else(|| AppError::NotFound("head commit not found".into()))?;

    let parent = parent_path_from(path);
    let name = path.rsplit_once('/').map(|(_, n)| n).unwrap_or("");
    let parent_fs_id = match resolve_fs_id(repos, repo_id, &head.root_id, parent).await {
        Ok(id) => id,
        Err(_) => return Ok(None),
    };
    let dir_data = match read_fs_dir_data(repos, repo_id, &parent_fs_id).await {
        Ok(d) => d,
        Err(_) => return Ok(None),
    };
    let entry = match dir_data.dirents.iter().find(|d| d.name == name) {
        Some(e) => e,
        None => return Ok(None),
    };
    Ok(Some((entry.mode & S_IFDIR != 0, entry.size, entry.mtime)))
}

/// Format a unix timestamp as an HTTP-date (RFC 1123) string.
pub fn http_date(ts: i64) -> String {
    chrono::DateTime::<chrono::Utc>::from_timestamp(ts, 0)
        .map(|dt| dt.format("%a, %d %b %Y %H:%M:%S GMT").to_string())
        .unwrap_or_default()
}

/// Build a `<D:href>` value for a repo path.
pub fn build_href(repo_id: &str, path: &str) -> String {
    let encoded = percent_encoding::utf8_percent_encode(path, HREF_ENCODE_SET).to_string();
    format!("/dav/{repo_id}{encoded}")
}

/// Join a parent path and a child name into an absolute path.
pub fn join_path(parent: &str, name: &str) -> String {
    if parent == "/" {
        format!("/{name}")
    } else {
        format!("{parent}/{name}")
    }
}
