use axum::{
    Json, Router,
    body::Body,
    extract::{Path, Query, State},
    http::StatusCode,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

use crate::AppState;
use crate::middleware::auth::SyncAuth;
use base::common::EMPTY_SHA1;
use base::error::AppError;
use infra::serialization::pack_fs;

/// A recv-fs pack is a batch of compressed fs objects, each capped on decompress.
const MAX_FS_PACK_BYTES: usize = 32 * 1024 * 1024;

/// Parse fs_ids: JSON array (newer seaf-daemon) or URL-encoded form (older).
fn parse_fs_ids_from_bytes(data: &[u8]) -> Result<Vec<String>, AppError> {
    if let Ok(arr) = serde_json::from_slice::<Vec<String>>(data)
        && !arr.is_empty()
    {
        return Ok(arr);
    }

    let body_str = String::from_utf8_lossy(data);
    let mut fs_ids = Vec::new();
    for pair in body_str.split('&') {
        let mut parts = pair.splitn(2, '=');
        let key = parts.next().unwrap_or("");
        let value = parts.next().unwrap_or("");
        if key == "fs_ids" && !value.is_empty() {
            fs_ids.push(percent_decode(value)?);
        }
    }
    Ok(fs_ids)
}

fn percent_decode(s: &str) -> Result<String, AppError> {
    let mut result = String::with_capacity(s.len());
    let mut chars = s.bytes();
    while let Some(b) = chars.next() {
        if b == b'%' {
            let hi = chars
                .next()
                .ok_or_else(|| AppError::BadRequest("truncated percent-encoding".into()))?;
            let lo = chars
                .next()
                .ok_or_else(|| AppError::BadRequest("truncated percent-encoding".into()))?;
            let byte = u8::from_str_radix(&String::from_utf8_lossy(&[hi, lo]), 16)
                .map_err(|_| AppError::BadRequest("invalid percent-encoding sequence".into()))?;
            result.push(byte as char);
        } else if b == b'+' {
            result.push(' ');
        } else {
            result.push(b as char);
        }
    }
    Ok(result)
}

#[derive(Deserialize)]
pub struct FsIdListQuery {
    #[serde(rename = "server-head")]
    pub server_head: Option<String>,
    #[serde(rename = "client-head")]
    pub client_head: Option<String>,
    #[serde(rename = "dir-only")]
    pub dir_only: Option<String>,
}

/// True for a 40-character hex object id (commit / fs object).
fn is_object_id(s: &str) -> bool {
    s.len() == 40 && s.bytes().all(|b| b.is_ascii_hexdigit())
}

pub fn fs_routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/{repo_id}/fs-id-list/", axum::routing::get(fs_id_list))
        .route("/{repo_id}/pack-fs/", axum::routing::post(pack_fs_handler))
        .route("/{repo_id}/check-fs/", axum::routing::post(check_fs))
        .route("/{repo_id}/recv-fs/", axum::routing::post(recv_fs))
}

pub async fn fs_id_list(
    State(state): State<Arc<AppState>>,
    auth: SyncAuth,
    Path(repo_id): Path<String>,
    Query(query): Query<FsIdListQuery>,
) -> Result<Json<Vec<String>>, AppError> {
    // Defense in depth: `SyncAuth` already fails closed when the URL repo does not
    // match the token's; every read path re-checks membership so a future gap cannot
    // leak data.
    crate::domain::permission::check_repo_read_permission(
        state.repos.member.as_ref(),
        &repo_id,
        auth.user_id,
    )
    .await?;

    let server_head = query
        .server_head
        .ok_or_else(|| AppError::BadRequest("missing server-head parameter".into()))?;

    // Upstream validates both heads as 40-hex object ids and answers 400
    // (`sync_api.go:118-132`); passing garbage through produced a 404/500.
    if !is_object_id(&server_head) {
        return Err(AppError::BadRequest(
            "Invalid server-head parameter.".into(),
        ));
    }
    if let Some(head) = query.client_head.as_deref()
        && !head.is_empty()
        && !is_object_id(head)
    {
        return Err(AppError::BadRequest(
            "Invalid client-head parameter.".into(),
        ));
    }

    let dir_only = !query.dir_only.as_deref().unwrap_or("").is_empty();
    if server_head == EMPTY_SHA1 {
        return Ok(Json(vec![]));
    }

    let svc = state.sync_service();
    let server_root = svc
        .get_commit_root(&repo_id, &server_head)
        .await?
        .ok_or_else(|| AppError::NotFound("server commit not found".into()))?;

    let client_root: Option<String> = match query.client_head.as_deref() {
        Some(head) if head != EMPTY_SHA1 => svc.get_commit_root(&repo_id, head).await?,
        _ => None,
    };

    if let Some(ref client_root) = client_root {
        if client_root == &server_root {
            return Ok(Json(vec![]));
        }
        let result: Vec<String> = svc
            .diff_fs_ids(&repo_id, client_root, &server_root, dir_only)
            .await?
            .into_iter()
            .collect();
        return Ok(Json(result));
    }

    let result: Vec<String> = svc
        .collect_fs_ids(&repo_id, &server_root, dir_only)
        .await?
        .into_iter()
        .collect();
    Ok(Json(result))
}

pub async fn pack_fs_handler(
    State(state): State<Arc<AppState>>,
    auth: SyncAuth,
    Path(repo_id): Path<String>,
    body: axum::body::Body,
) -> Result<axum::response::Response, AppError> {
    // Membership check before the body is read (see `fs_id_list`).
    crate::domain::permission::check_repo_read_permission(
        state.repos.member.as_ref(),
        &repo_id,
        auth.user_id,
    )
    .await?;

    let body_data = axum::body::to_bytes(body, 10 * 1024 * 1024)
        .await
        .map_err(|e| AppError::internal(e.to_string()))?;
    let fs_ids = parse_fs_ids_from_bytes(&body_data)?;

    let objects = state
        .sync_service()
        .fetch_fs_objects(&repo_id, &fs_ids)
        .await?;

    // A requested id the server cannot serve must fail the request, not produce a short
    // 200: seaf-daemon retries "requested but not returned" immediately with no sleep
    // and no cap, so a silent subset becomes a client-side infinite loop. Upstream
    // `packFSCB` returns 500 when an object cannot be read. `EMPTY_SHA1` (empty dir /
    // zero-byte file) has no stored object and is answered with no entry as before;
    // `fs-id-list` never advertises it (see `diff_fs_ids`).
    let stored: std::collections::HashSet<&str> =
        objects.iter().map(|obj| obj.fs_id.as_str()).collect();
    if let Some(missing) = fs_ids
        .iter()
        .find(|id| id.as_str() != EMPTY_SHA1 && !stored.contains(id.as_str()))
    {
        return Err(AppError::Internal(format!(
            "fs object {missing} is missing from library {repo_id}"
        )));
    }

    // zlib compression is CPU-bound; offload to the blocking pool.
    let packed = tokio::task::spawn_blocking(move || -> Result<Vec<u8>, std::io::Error> {
        let obj_map: std::collections::HashMap<&str, &infra::entity::fs_object::Model> = objects
            .iter()
            .map(|obj| (obj.fs_id.as_str(), obj))
            .collect();
        let mut entries = Vec::new();
        for fs_id in &fs_ids {
            if let Some(obj) = obj_map.get(fs_id.as_str()) {
                let compressed = pack_fs::compress_fs_data(obj.data.as_bytes())?;
                entries.push((obj.fs_id.clone(), compressed));
            }
        }
        Ok(pack_fs::encode_pack_fs_entries(&entries))
    })
    .await
    .map_err(|e| AppError::internal(e.to_string()))?
    .map_err(|e| AppError::internal(e.to_string()))?;

    axum::response::Response::builder()
        .header("Content-Type", "application/octet-stream")
        .body(Body::from(packed))
        .map_err(|e| AppError::internal(e.to_string()))
}

#[derive(Serialize)]
pub struct CheckFsResponse {
    pub missing: Vec<String>,
}

pub async fn check_fs(
    State(state): State<Arc<AppState>>,
    auth: SyncAuth,
    Path(repo_id): Path<String>,
    body: axum::body::Body,
) -> Result<Json<Vec<String>>, AppError> {
    // Membership check before the body is read (see `fs_id_list`).
    crate::domain::permission::check_repo_read_permission(
        state.repos.member.as_ref(),
        &repo_id,
        auth.user_id,
    )
    .await?;

    let body_data = axum::body::to_bytes(body, 10 * 1024 * 1024)
        .await
        .map_err(|e| AppError::internal(e.to_string()))?;
    let fs_ids = parse_fs_ids_from_bytes(&body_data)?;

    // Only fs_ids are needed: a column-projected, chunked query beats reading `data`.
    let existing = state
        .repos
        .fs_object
        .find_existing_fs_ids(&repo_id, &fs_ids)
        .await?;

    // seafile's `seaf_fs_manager_object_exists()` treats EMPTY_SHA1 as always
    // present ("Empty file and dir always exists"), so never report it missing.
    let missing: Vec<String> = fs_ids
        .iter()
        .filter(|fs_id| fs_id.as_str() != EMPTY_SHA1 && !existing.contains(fs_id.as_str()))
        .cloned()
        .collect();

    Ok(Json(missing))
}

pub async fn recv_fs(
    State(state): State<Arc<AppState>>,
    auth: SyncAuth,
    Path(repo_id): Path<String>,
    body: axum::body::Body,
) -> Result<StatusCode, AppError> {
    // A read-only member must not inject fs objects; checked before the body is read.
    crate::domain::permission::check_repo_write_permission(
        state.repos.member.as_ref(),
        &repo_id,
        auth.user_id,
    )
    .await?;

    let data = axum::body::to_bytes(body, MAX_FS_PACK_BYTES)
        .await
        .map_err(|e| AppError::internal(e.to_string()))?;

    // A malformed pack is a client error that upstream answers with 400
    // (`sync_api.go:465-509`), not a 500.
    let entries = pack_fs::decode_pack_fs_entries(&data).map_err(AppError::BadRequest)?;
    state
        .sync_service()
        .insert_fs_objects(&repo_id, entries)
        .await?;
    Ok(StatusCode::OK)
}
