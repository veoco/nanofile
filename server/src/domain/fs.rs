//! FS domain logic — seafile-compatible FS object serialization and fs_id
//! computation.
//!
//! The `FsDirData`, `FsFileData`, `DirEntryData` types are defined in
//! `base::common`; this module provides the associated computation functions
//! (JSON serialization + sha1 fs_id).
//!
//! The exact byte representation matters: seafile hashes the JSON string into
//! the FS object id, so it must match `json_dumps(obj, JSON_SORT_KEYS)` byte
//! for byte (sorted keys, `": "` / `", "` separators, and `modifier`/`size`
//! only on regular-file dirents).

use base::common::{DirEntryData, FsDirData, FsFileData};
use sha1::{Digest, Sha1};

/// `S_IFMT` / `S_IFREG` type bits, used like C's `S_ISREG()`.
const S_IFMT: i32 = 0o170000;
const S_IFREG_TYPE: i32 = 0o100000;

/// Quote a string the way jansson does by default (no `JSON_ENSURE_ASCII`):
/// UTF-8 passes through and only `"`, `\` and control characters are escaped.
/// `serde_json`'s escaping matches jansson for everything that can appear in an
/// FS object (ids are hex, filenames are validated without control chars).
fn json_string(s: &str) -> String {
    serde_json::to_string(s).expect("serializing a string cannot fail")
}

/// Serialize one directory entry like seafile's `add_to_dirent_array()`:
/// alphabetical keys (`JSON_SORT_KEYS`) and `modifier`/`size` present **only
/// for regular files** (`S_ISREG`).
fn dirent_to_json(d: &DirEntryData) -> String {
    let is_regular = d.mode & S_IFMT == S_IFREG_TYPE;

    let mut out = String::with_capacity(64 + d.name.len() + d.modifier.len());
    out.push_str("{\"id\": ");
    out.push_str(&json_string(&d.id));
    out.push_str(", \"mode\": ");
    out.push_str(&d.mode.to_string());
    if is_regular {
        out.push_str(", \"modifier\": ");
        out.push_str(&json_string(&d.modifier));
    }
    out.push_str(", \"mtime\": ");
    out.push_str(&d.mtime.to_string());
    out.push_str(", \"name\": ");
    out.push_str(&json_string(&d.name));
    if is_regular {
        out.push_str(", \"size\": ");
        out.push_str(&d.size.to_string());
    }
    out.push('}');
    out
}

/// Serialize a directory FS object exactly like seafile's `seaf_dir_to_json()`
/// (`json_dumps(obj, JSON_SORT_KEYS)`).
pub fn dir_to_json(data: &FsDirData) -> String {
    let mut out = String::with_capacity(64);
    out.push_str("{\"dirents\": [");
    for (i, d) in data.dirents.iter().enumerate() {
        if i > 0 {
            out.push_str(", ");
        }
        out.push_str(&dirent_to_json(d));
    }
    out.push_str("], \"type\": ");
    out.push_str(&data.obj_type.to_string());
    out.push_str(", \"version\": ");
    out.push_str(&data.version.to_string());
    out.push('}');
    out
}

/// Serialize a file FS object exactly like seafile's `create_seafile_json()`
/// (`json_dumps(obj, JSON_SORT_KEYS)`).
pub fn file_to_json(data: &FsFileData) -> String {
    let mut out = String::with_capacity(64 + data.block_ids.len() * 44);
    out.push_str("{\"block_ids\": [");
    for (i, id) in data.block_ids.iter().enumerate() {
        if i > 0 {
            out.push_str(", ");
        }
        out.push_str(&json_string(id));
    }
    out.push_str("], \"size\": ");
    out.push_str(&data.size.to_string());
    out.push_str(", \"type\": ");
    out.push_str(&data.obj_type.to_string());
    out.push_str(", \"version\": ");
    out.push_str(&data.version.to_string());
    out.push('}');
    out
}

/// Compute the SHA1 hex digest (fs_id) of an FS object JSON string.
///
/// This is the core identity function for seafile FS objects:
/// `fs_id = sha1_hex(json_dumps(obj, JSON_SORT_KEYS))`.
pub fn compute_fs_id(json: &str) -> String {
    let mut hasher = Sha1::new();
    hasher.update(json.as_bytes());
    hex::encode(hasher.finalize())
}

/// Compute the fs_id and JSON for a directory.
///
/// Returns `None` for empty directories: their id is the `EMPTY_SHA1` sentinel
/// and no object is persisted (`seaf_dir_save()` skips them).
pub fn compute_dir(data: &FsDirData) -> Option<(String, String)> {
    if data.dirents.is_empty() {
        return None;
    }
    let json = dir_to_json(data);
    let fs_id = compute_fs_id(&json);
    Some((fs_id, json))
}

/// Compute the fs_id and JSON for a file.
///
/// Returns `None` for zero-byte files: their id is the `EMPTY_SHA1` sentinel
/// and no `seafile` object exists (`seaf_fs_manager_index_blocks()` sets the id
/// to all zeros for a zero-size file).
pub fn compute_file(data: &FsFileData) -> Option<(String, String)> {
    if data.size == 0 {
        return None;
    }
    let json = file_to_json(data);
    let fs_id = compute_fs_id(&json);
    Some((fs_id, json))
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
