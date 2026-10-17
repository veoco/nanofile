//! Serialization and fs_id computation for seafile-compatible FS objects
//! (`FsDirData`/`FsFileData`/`DirEntryData` live in `base::common`). Bytes
//! matter: seafile hashes this JSON into the object id, so it must match
//! `json_dumps(obj, JSON_SORT_KEYS)` exactly — sorted keys, and `modifier`/`size`
//! present only on regular-file dirents.

use base::common::{DirEntryData, FsDirData, FsFileData};
use sha1::{Digest, Sha1};

/// `S_IFMT` / `S_IFREG` type bits, used like C's `S_ISREG()`.
const S_IFMT: i32 = 0o170000;
const S_IFREG_TYPE: i32 = 0o100000;

/// Quote a string the way jansson does by default (no `JSON_ENSURE_ASCII`): UTF-8
/// passes through and only `"`, `\` and control characters are escaped, matching
/// `serde_json` for everything that can appear in an FS object.
fn json_string(s: &str) -> String {
    serde_json::to_string(s).expect("serializing a string cannot fail")
}

/// Serialize one directory entry like `add_to_dirent_array()`: sorted keys, and
/// `modifier`/`size` present **only for regular files** (`S_ISREG`).
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

/// Serialize a directory object like `seaf_dir_to_json()` (sorted keys).
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

/// Serialize a file object like `create_seafile_json()` (sorted keys).
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

/// The fs_id of an FS object: `sha1_hex(json_dumps(obj, JSON_SORT_KEYS))`.
pub fn compute_fs_id(json: &str) -> String {
    let mut hasher = Sha1::new();
    hasher.update(json.as_bytes());
    hex::encode(hasher.finalize())
}

/// Sort directory entries into seafile's canonical order: descending by name,
/// i.e. upstream's `compare_dirents()` (`strcmp(b, a)` in `common/fs-mgr.c`),
/// as every official client writes them (`daemon/vc-utils.c`).
///
/// `fs_id` is the SHA-1 of the serialized object, so the order *is* the identity;
/// write paths building `dirents` in insertion order must sort first, and the
/// merge sorts its inputs and output the same way.
pub fn sort_dirents_desc(dirents: &mut [DirEntryData]) {
    dirents.sort_by(|a, b| b.name.cmp(&a.name));
}

/// Compute the fs_id and JSON for a directory; `None` for empty ones, whose id is
/// the `EMPTY_SHA1` sentinel and which are not persisted (`seaf_dir_save()` skips
/// them). Entries are hashed in the order given — re-sorting here would change an
/// id already on disk.
pub fn compute_dir(data: &FsDirData) -> Option<(String, String)> {
    if data.dirents.is_empty() {
        return None;
    }
    let json = dir_to_json(data);
    let fs_id = compute_fs_id(&json);
    Some((fs_id, json))
}

/// Compute the fs_id and JSON for a file; `None` for zero-byte ones, whose id is
/// the `EMPTY_SHA1` sentinel and which have no object
/// (`seaf_fs_manager_index_blocks()` sets the id to zeros).
pub fn compute_file(data: &FsFileData) -> Option<(String, String)> {
    if data.size == 0 {
        return None;
    }
    let json = file_to_json(data);
    let fs_id = compute_fs_id(&json);
    Some((fs_id, json))
}

/// Whether the in-repo path `dst` is `src` itself or lies inside `src`, used to
/// reject "move a directory into its own subtree".
///
/// The FS tree update is two-phase (remove from the old parent, commit, add to the
/// new one), so a destination inside the moved subtree is already gone: phase 2
/// fails *after* the subtree left HEAD, leaving no trash entry and a 500. Both
/// arguments are canonical in-repo paths from `base::sanitize::safe_normalize_path`.
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
