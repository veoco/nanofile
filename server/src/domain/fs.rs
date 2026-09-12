//! FS domain logic — serialization to compact JSON and fs_id computation.
//!
//! The `FsDirData`, `FsFileData`, `DirEntryData` types are defined in
//! `base::common`; this module provides the associated computation
//! functions (to_compact_json, sha1 fs_id) that were previously methods
//! on those types in `infra::serialization::fs_json`.

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

// ── Note: store_dir_data / store_file_data moved to fs::core::store ─────
// These were previously here but have been moved to `crate::fs::core::store`
// (store_fs_dir_object / store_fs_file_object) to keep domain pure.
// This module now contains only pure computation functions.

/// Whether the in-repo path `dst` is `src` itself or lies inside `src`.
///
/// Used to reject "move a directory into its own subtree". The FS tree update
/// is a two-phase (remove-from-old-parent, then add-to-new-parent) sequence
/// with a commit in between, so a destination inside the moved subtree no
/// longer exists once phase 1 is durable: phase 2 then fails *after* the
/// subtree has already disappeared from HEAD, leaving no trash entry and a 500.
/// The WebDAV MOVE/COPY handler guarded this case; the REST and sync move
/// endpoints did not.
///
/// Both arguments are canonical in-repo paths (`/`-rooted, no `..`/`.` and no
/// trailing slash except for the root itself), as produced by
/// `base::sanitize::safe_normalize_path`.
pub fn is_self_or_subpath(src: &str, dst: &str) -> bool {
    if dst == src {
        return true;
    }
    let prefix = if src == "/" {
        "/".to_string()
    } else {
        format!("{}/", src.trim_end_matches('/'))
    };
    dst.starts_with(&prefix)
}

#[cfg(test)]
mod tests {
    use super::is_self_or_subpath;

    #[test]
    fn identical_paths_are_self() {
        assert!(is_self_or_subpath("/a/b", "/a/b"));
        assert!(is_self_or_subpath("/", "/"));
    }

    #[test]
    fn descendants_are_subpaths() {
        assert!(is_self_or_subpath("/a", "/a/b"));
        assert!(is_self_or_subpath("/a", "/a/b/c"));
        assert!(is_self_or_subpath("/", "/anything"));
    }

    #[test]
    fn siblings_and_prefix_traps_are_not_subpaths() {
        assert!(!is_self_or_subpath("/a", "/ab"));
        assert!(!is_self_or_subpath("/a/b", "/a/c"));
        assert!(!is_self_or_subpath("/a/b", "/a"));
    }
}
