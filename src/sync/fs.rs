use axum::{
    Json, Router,
    body::Body,
    extract::{Path, Query, State},
    http::StatusCode,
};
use sea_orm::{ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::sync::Arc;

use crate::AppState;
use crate::auth::middleware::SyncAuth;
use crate::entity::{commit, fs_object};
use crate::error::AppError;
use crate::serialization::fs_json::SEAF_METADATA_TYPE_DIR;
use crate::serialization::pack_fs;

/// Parse fs_ids from the request body. The seaf-daemon may send either:
/// 1. JSON array: ["id1", "id2"] (newer versions)
/// 2. URL-encoded form: fs_ids=id1&fs_ids=id2 (older versions)
fn parse_fs_ids_from_bytes(data: &[u8]) -> Vec<String> {
    // Try JSON array first
    if let Ok(arr) = serde_json::from_slice::<Vec<String>>(data)
        && !arr.is_empty()
    {
        return arr;
    }

    // Fall back to URL-encoded form
    let body_str = String::from_utf8_lossy(data);
    let mut fs_ids = Vec::new();
    for pair in body_str.split('&') {
        let mut parts = pair.splitn(2, '=');
        let key = parts.next().unwrap_or("");
        let value = parts.next().unwrap_or("");
        if key == "fs_ids" && !value.is_empty() {
            let decoded = percent_decode(value);
            fs_ids.push(decoded);
        }
    }
    fs_ids
}

fn percent_decode(s: &str) -> String {
    let mut result = String::with_capacity(s.len());
    let mut chars = s.bytes();
    while let Some(b) = chars.next() {
        if b == b'%' {
            let hi = chars.next().unwrap_or(b'0');
            let lo = chars.next().unwrap_or(b'0');
            if let Ok(byte) = u8::from_str_radix(&String::from_utf8_lossy(&[hi, lo]), 16) {
                result.push(byte as char);
            }
        } else if b == b'+' {
            result.push(' ');
        } else {
            result.push(b as char);
        }
    }
    result
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

#[derive(Deserialize)]
pub struct PackFsRequest {
    pub fs_ids: Vec<String>,
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

    let server_commit = commit::Entity::find()
        .filter(commit::Column::RepoId.eq(&repo_id))
        .filter(commit::Column::CommitId.eq(&server_head))
        .one(state.db.as_ref())
        .await?
        .ok_or_else(|| AppError::NotFound("server commit not found".into()))?;

    let server_root = server_commit.root_id;

    if let Some(ref client_head) = query.client_head {
        if client_head == "0000000000000000000000000000000000000000" {
            let mut collected = HashSet::new();
            collect_fs_ids_recursive(state.db.as_ref(), &repo_id, &server_root, &mut collected)
                .await?;
            let result = filter_collected(collected, dir_only, state.db.as_ref(), &repo_id).await?;
            return Ok(Json(result));
        }

        let client_commit = commit::Entity::find()
            .filter(commit::Column::RepoId.eq(&repo_id))
            .filter(commit::Column::CommitId.eq(client_head.as_str()))
            .one(state.db.as_ref())
            .await?;

        if let Some(client_commit) = client_commit
            && client_commit.root_id == server_root
        {
            return Ok(Json(vec![]));
        }

        let mut collected = HashSet::new();
        collect_fs_ids_recursive(state.db.as_ref(), &repo_id, &server_root, &mut collected).await?;
        let result = filter_collected(collected, dir_only, state.db.as_ref(), &repo_id).await?;
        Ok(Json(result))
    } else {
        let mut collected = HashSet::new();
        collect_fs_ids_recursive(state.db.as_ref(), &repo_id, &server_root, &mut collected).await?;
        let result = filter_collected(collected, dir_only, state.db.as_ref(), &repo_id).await?;
        Ok(Json(result))
    }
}

/// When dir_only is true, filter to only include directory object IDs
/// (obj_type == SEAF_METADATA_TYPE_DIR). When false, return all IDs as-is.
async fn filter_collected(
    collected: HashSet<String>,
    dir_only: bool,
    db: &DatabaseConnection,
    repo_id: &str,
) -> Result<Vec<String>, AppError> {
    if !dir_only {
        let result: Vec<String> = collected.into_iter().collect();
        return Ok(result);
    }
    let mut dir_ids = Vec::new();
    for id in collected {
        let obj = fs_object::Entity::find()
            .filter(fs_object::Column::RepoId.eq(repo_id))
            .filter(fs_object::Column::FsId.eq(&id))
            .one(db)
            .await?;
        if let Some(obj) = obj
            && obj.obj_type == SEAF_METADATA_TYPE_DIR as i8
        {
            dir_ids.push(id);
        }
    }
    Ok(dir_ids)
}

async fn collect_fs_ids_recursive(
    db: &DatabaseConnection,
    repo_id: &str,
    fs_id: &str,
    collected: &mut HashSet<String>,
) -> Result<(), AppError> {
    if collected.contains(fs_id) {
        return Ok(());
    }

    let fs_obj = fs_object::Entity::find()
        .filter(fs_object::Column::RepoId.eq(repo_id))
        .filter(fs_object::Column::FsId.eq(fs_id))
        .one(db)
        .await?;

    if let Some(fs_obj) = fs_obj {
        collected.insert(fs_id.to_string());

        if fs_obj.obj_type == SEAF_METADATA_TYPE_DIR as i8 {
            let dir_data: crate::serialization::fs_json::FsDirData =
                serde_json::from_str(&fs_obj.data)
                    .map_err(|e| AppError::Internal(format!("invalid dir data: {}", e)))?;

            for entry in &dir_data.dirents {
                Box::pin(collect_fs_ids_recursive(db, repo_id, &entry.id, collected)).await?;
            }
        }
    }

    Ok(())
}

pub async fn pack_fs_handler(
    State(state): State<Arc<AppState>>,
    _auth: SyncAuth,
    Path(repo_id): Path<String>,
    body: axum::body::Body,
) -> Result<axum::response::Response, AppError> {
    // Read body bytes
    let body_data = axum::body::to_bytes(body, 10 * 1024 * 1024)
        .await
        .map_err(|e| AppError::Internal(e.to_string()))?;
    let fs_ids = parse_fs_ids_from_bytes(&body_data);

    let mut entries = Vec::new();
    for fs_id in &fs_ids {
        let obj = fs_object::Entity::find()
            .filter(fs_object::Column::RepoId.eq(&repo_id))
            .filter(fs_object::Column::FsId.eq(fs_id))
            .one(state.db.as_ref())
            .await?;

        if let Some(obj) = obj {
            // Compress JSON to zlib for wire format compatibility
            let compressed = pack_fs::compress_fs_data(obj.data.as_bytes())
                .map_err(|e| AppError::Internal(e.to_string()))?;
            entries.push((obj.fs_id, compressed));
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
    let fs_ids = parse_fs_ids_from_bytes(&body_data);

    let mut missing = Vec::new();
    for fs_id in &fs_ids {
        let exists = fs_object::Entity::find()
            .filter(fs_object::Column::RepoId.eq(&repo_id))
            .filter(fs_object::Column::FsId.eq(fs_id))
            .one(state.db.as_ref())
            .await?
            .is_some();

        if !exists {
            missing.push(fs_id.clone());
        }
    }

    Ok(Json(missing))
}

pub async fn recv_fs(
    State(state): State<Arc<AppState>>,
    _auth: SyncAuth,
    Path(repo_id): Path<String>,
    body: axum::body::Body,
) -> Result<StatusCode, AppError> {
    let data = axum::body::to_bytes(body, usize::MAX)
        .await
        .map_err(|e| AppError::Internal(e.to_string()))?;

    let entries = pack_fs::decode_pack_fs_entries(&data).map_err(AppError::Internal)?;

    for (fs_id, obj_data) in entries {
        let existing = fs_object::Entity::find()
            .filter(fs_object::Column::RepoId.eq(&repo_id))
            .filter(fs_object::Column::FsId.eq(&fs_id))
            .one(state.db.as_ref())
            .await?;

        if existing.is_none() {
            // Decompress incoming zlib and store as JSON text
            let decompressed = pack_fs::decompress_fs_data(&obj_data)
                .map_err(|e| AppError::Internal(e.to_string()))?;
            let json_str =
                String::from_utf8(decompressed).map_err(|e| AppError::Internal(e.to_string()))?;

            let json_val: serde_json::Value =
                serde_json::from_str(&json_str).map_err(|e| AppError::Internal(e.to_string()))?;
            let obj_type = json_val.get("type").and_then(|v| v.as_i64()).unwrap_or(1) as i8;

            let fs_obj = fs_object::ActiveModel {
                id: sea_orm::NotSet,
                repo_id: sea_orm::Set(repo_id.clone()),
                fs_id: sea_orm::Set(fs_id),
                obj_type: sea_orm::Set(obj_type),
                data: sea_orm::Set(json_str),
            };
            fs_object::Entity::insert(fs_obj)
                .exec(state.db.as_ref())
                .await?;
        }
    }

    Ok(StatusCode::OK)
}
