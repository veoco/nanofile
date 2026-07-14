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
use base::error::AppError;
use infra::serialization::pack_fs;

/// Parse fs_ids from the request body. The seaf-daemon may send either:
/// 1. JSON array: ["id1", "id2"] (newer versions)
/// 2. URL-encoded form: fs_ids=id1&fs_ids=id2 (older versions)
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

pub fn fs_routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/{repo_id}/fs-id-list/", axum::routing::get(fs_id_list))
        .route("/{repo_id}/pack-fs/", axum::routing::post(pack_fs_handler))
        .route("/{repo_id}/check-fs/", axum::routing::post(check_fs))
        .route("/{repo_id}/recv-fs/", axum::routing::post(recv_fs))
}

pub async fn fs_id_list(
    State(state): State<Arc<AppState>>,
    _auth: SyncAuth,
    Path(repo_id): Path<String>,
    Query(query): Query<FsIdListQuery>,
) -> Result<Json<Vec<String>>, AppError> {
    let server_head = query
        .server_head
        .ok_or_else(|| AppError::BadRequest("missing server-head parameter".into()))?;

    let dir_only = !query.dir_only.as_deref().unwrap_or("").is_empty();
    let empty_hash = "0000000000000000000000000000000000000000";
    if server_head == empty_hash {
        return Ok(Json(vec![]));
    }

    let svc = state.sync_service();
    let server_root = svc
        .get_commit_root(&repo_id, &server_head)
        .await?
        .ok_or_else(|| AppError::NotFound("server commit not found".into()))?;

    if let Some(ref client_head) = query.client_head {
        if client_head == empty_hash {
            let collected = svc.collect_fs_ids(&repo_id, &server_root).await?;
            let result = if dir_only {
                svc.filter_dir_ids(&repo_id, &collected).await?
            } else {
                collected.into_iter().collect()
            };
            return Ok(Json(result));
        }

        if let Some(client_root) = svc.get_commit_root(&repo_id, client_head).await?
            && client_root == server_root
        {
            return Ok(Json(vec![]));
        }
    }

    let collected = svc.collect_fs_ids(&repo_id, &server_root).await?;
    let result = if dir_only {
        svc.filter_dir_ids(&repo_id, &collected).await?
    } else {
        collected.into_iter().collect()
    };
    Ok(Json(result))
}

pub async fn pack_fs_handler(
    State(state): State<Arc<AppState>>,
    _auth: SyncAuth,
    Path(repo_id): Path<String>,
    body: axum::body::Body,
) -> Result<axum::response::Response, AppError> {
    let body_data = axum::body::to_bytes(body, 10 * 1024 * 1024)
        .await
        .map_err(|e| AppError::Internal(e.to_string()))?;
    let fs_ids = parse_fs_ids_from_bytes(&body_data)?;

    let objects = state
        .sync_service()
        .fetch_fs_objects(&repo_id, &fs_ids)
        .await?;

    let obj_map: std::collections::HashMap<&str, &infra::entity::fs_object::Model> = objects
        .iter()
        .map(|obj| (obj.fs_id.as_str(), obj))
        .collect();

    let mut entries = Vec::new();
    for fs_id in &fs_ids {
        if let Some(obj) = obj_map.get(fs_id.as_str()) {
            let compressed = pack_fs::compress_fs_data(obj.data.as_bytes())
                .map_err(|e| AppError::Internal(e.to_string()))?;
            entries.push((obj.fs_id.clone(), compressed));
        }
    }
    let packed = pack_fs::encode_pack_fs_entries(&entries);

    let response = axum::response::Response::builder()
        .header("Content-Type", "application/octet-stream")
        .body(Body::from(packed))
        .map_err(|e| AppError::Internal(e.to_string()))?;
    Ok(response)
}

#[derive(Serialize)]
pub struct CheckFsResponse {
    pub missing: Vec<String>,
}

pub async fn check_fs(
    State(state): State<Arc<AppState>>,
    _auth: SyncAuth,
    Path(repo_id): Path<String>,
    body: axum::body::Body,
) -> Result<Json<Vec<String>>, AppError> {
    let body_data = axum::body::to_bytes(body, 10 * 1024 * 1024)
        .await
        .map_err(|e| AppError::Internal(e.to_string()))?;
    let fs_ids = parse_fs_ids_from_bytes(&body_data)?;

    let existing = state
        .sync_service()
        .fetch_fs_objects(&repo_id, &fs_ids)
        .await?;

    let existing_set: std::collections::HashSet<&str> =
        existing.iter().map(|obj| obj.fs_id.as_str()).collect();

    let missing: Vec<String> = fs_ids
        .iter()
        .filter(|fs_id| !existing_set.contains(fs_id.as_str()))
        .cloned()
        .collect();

    Ok(Json(missing))
}

pub async fn recv_fs(
    State(state): State<Arc<AppState>>,
    _auth: SyncAuth,
    Path(repo_id): Path<String>,
    body: axum::body::Body,
) -> Result<StatusCode, AppError> {
    let max_bytes = (state.config.server.max_upload_size_mb * 1024 * 1024) as usize;
    let data = axum::body::to_bytes(body, max_bytes)
        .await
        .map_err(|e| AppError::Internal(e.to_string()))?;

    let entries = pack_fs::decode_pack_fs_entries(&data).map_err(AppError::Internal)?;
    state
        .sync_service()
        .insert_fs_objects(&repo_id, entries)
        .await?;
    Ok(StatusCode::OK)
}
