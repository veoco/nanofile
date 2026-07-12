//! FS domain logic — serialization to compact JSON and fs_id computation.
//!
//! The `FsDirData`, `FsFileData`, `DirEntryData` types are defined in
//! `base::common`; this module provides the associated computation
//! functions (to_compact_json, sha1 fs_id) that were previously methods
//! on those types in `infra::serialization::fs_json`.

use base::common::EMPTY_SHA1;
use base::common::{FsDirData, FsFileData};
use sea_orm::{ConnectionTrait, DatabaseBackend, DatabaseConnection, Statement};
use sha1::{Digest, Sha1};

use base::error::AppError;

/// Serialize a directory FS object to compact JSON (no extra whitespace).
pub fn dir_to_compact_json(data: &FsDirData) -> String {
    serde_json::json!({
        "dirents": data.dirents,
        "type": data.obj_type,
        "version": data.version,
    })
    .to_string()
}

/// Serialize a file FS object to compact JSON (no extra whitespace).
pub fn file_to_compact_json(data: &FsFileData) -> String {
    serde_json::json!({
        "block_ids": data.block_ids,
        "size": data.size,
        "type": data.obj_type,
        "version": data.version,
    })
    .to_string()
}

/// Compute the SHA1 hex digest (fs_id) of a compact JSON string.
///
/// This is the core identity function for seafile FS objects:
/// fs_id = sha1_hex(compact_json)
pub fn compute_fs_id(json: &str) -> String {
    let mut hasher = Sha1::new();
    hasher.update(json.as_bytes());
    hex::encode(hasher.finalize())
}

/// Compute the fs_id and compact JSON for a directory.
///
/// Returns `None` for empty directories (they use the EMPTY_SHA1 sentinel
/// and are never stored, matching seafile convention).
pub fn compute_dir(data: &FsDirData) -> Option<(String, String)> {
    if data.dirents.is_empty() {
        return None;
    }
    let json = dir_to_compact_json(data);
    let fs_id = compute_fs_id(&json);
    Some((fs_id, json))
}

/// Compute the fs_id and compact JSON for a file.
pub fn compute_file(data: &FsFileData) -> (String, String) {
    let json = file_to_compact_json(data);
    let fs_id = compute_fs_id(&json);
    (fs_id, json)
}

/// Check if an fs_id is the empty sentinel (all zeros).
pub fn is_empty_fs_id(fs_id: &str) -> bool {
    fs_id == EMPTY_SHA1
}

// ── Persistence helpers (bridge domain + infra) ──────────────────────────
//
// These functions compute the fs_id and compact JSON, then INSERT the
// object into fs_objects.  The `compute_*` functions above are the pure
// domain logic; these convenience wrappers combine compute + store in a
// single async call for callers that need both steps.

/// Compute fs_id, serialize, and INSERT OR IGNORE into fs_objects.
/// Returns the fs_id (or `EMPTY_SHA1` for empty directories).
pub async fn store_dir_data(
    db: &DatabaseConnection,
    repo_id: &str,
    data: &FsDirData,
) -> Result<String, AppError> {
    if data.dirents.is_empty() {
        return Ok(EMPTY_SHA1.to_string());
    }
    let (fs_id, json) = compute_dir(data).expect("non-empty directory");
    let _ = db
        .execute(Statement::from_sql_and_values(
            DatabaseBackend::Sqlite,
            "INSERT OR IGNORE INTO fs_objects (repo_id, fs_id, obj_type, data) VALUES ($1, $2, $3, $4)",
            vec![
                repo_id.to_owned().into(),
                fs_id.clone().into(),
                (data.obj_type as i8).into(),
                json.into(),
            ],
        ))
        .await?;
    Ok(fs_id)
}

/// Compute fs_id, serialize, and INSERT OR IGNORE into fs_objects.
/// Returns the fs_id.
pub async fn store_file_data(
    db: &DatabaseConnection,
    repo_id: &str,
    data: &FsFileData,
) -> Result<String, AppError> {
    let (fs_id, json) = compute_file(data);
    let _ = db
        .execute(Statement::from_sql_and_values(
            DatabaseBackend::Sqlite,
            "INSERT OR IGNORE INTO fs_objects (repo_id, fs_id, obj_type, data) VALUES ($1, $2, $3, $4)",
            vec![
                repo_id.to_owned().into(),
                fs_id.clone().into(),
                (data.obj_type as i8).into(),
                json.into(),
            ],
        ))
        .await?;
    Ok(fs_id)
}
