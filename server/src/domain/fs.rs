//! FS domain logic — serialization to compact JSON and fs_id computation.
//!
//! The `FsDirData`, `FsFileData`, `DirEntryData` types are defined in
//! `base::common`; this module provides the associated computation
//! functions (to_compact_json, sha1 fs_id) that were previously methods
//! on those types in `infra::serialization::fs_json`.

use base::common::EMPTY_SHA1;
use base::common::{FsDirData, FsFileData};
use sha1::{Digest, Sha1};

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

// ── Note: store_dir_data / store_file_data moved to fs::core::store ─────
// These were previously here but have been moved to `crate::fs::core::store`
// (store_fs_dir_object / store_fs_file_object) to keep domain pure.
// This module now contains only pure computation functions.
