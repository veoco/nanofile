//! Shared constants and types for the nanofile ecosystem.
//!
//! This module is split across crate boundaries:
//! - The actual code lives here in `base`
//! - `infra` re-exports it and adds `util` functions
//! - `server` accesses everything via `crate::common::*`

/// SHA1 sentinel for empty directories (seafile convention).
/// seafile-server's seaf_dir_new() forces this when entries are NULL;
/// seaf_dir_save() skips persistence for this value.
pub const EMPTY_SHA1: &str = "0000000000000000000000000000000000000000";

/// S_IFREG (0100644) — regular file.
pub const S_IFREG: i32 = 33188;

/// S_IFDIR (040000) — directory.
pub const S_IFDIR: i32 = 16384;

/// SeafMetadataType: file = 1
pub const SEAF_METADATA_TYPE_FILE: i32 = 1;

/// SeafMetadataType: dir = 3
pub const SEAF_METADATA_TYPE_DIR: i32 = 3;

/// Directory entry returned by API listing endpoints.
#[derive(Debug, serde::Serialize)]
pub struct DirEntry {
    pub id: String,
    #[serde(rename = "type")]
    pub entry_type: String,
    pub name: String,
    pub size: i64,
    pub mtime: i64,
    pub permission: String,
    /// Last modifier email (empty string if unknown). Files only in the
    /// original seafile protocol, but we store it for all entry types.
    #[serde(default)]
    pub modifier: String,
    /// Parent directory path. Present only in recursive listings.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_dir: Option<String>,
    /// Modifier display name. Present only for file entries in recursive listings.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub modifier_name: Option<String>,
    /// Modifier contact email. Present only for file entries in recursive listings.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub modifier_contact_email: Option<String>,
}

// ── FS object types (Seafile storage format) ──────────────────────────────

/// A single entry in a directory listing (storage format).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct DirEntryData {
    pub id: String,
    pub mode: i32,
    /// Who last modified this entry. Only included for files in seafile's
    /// format. May be missing in FS objects synced from seaf-daemon.
    #[serde(default)]
    pub modifier: String,
    pub mtime: i64,
    pub name: String,
    /// Only included for files in seafile's format.
    /// May be missing for directory entries from seaf-daemon.
    #[serde(default)]
    pub size: i64,
}

/// Directory FS object (storage format stored in fs_objects.data as JSON).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct FsDirData {
    pub dirents: Vec<DirEntryData>,
    #[serde(rename = "type")]
    pub obj_type: i32,
    pub version: i32,
}

/// File FS object (storage format stored in fs_objects.data as JSON).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct FsFileData {
    pub block_ids: Vec<String>,
    pub size: i64,
    #[serde(rename = "type")]
    pub obj_type: i32,
    pub version: i32,
}

/// Commit metadata (storage format).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct CommitData {
    pub commit_id: String,
    pub repo_id: String,
    pub root_id: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub creator_name: String,
    pub creator: String,
    pub description: String,
    pub ctime: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub second_parent_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub repo_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub repo_desc: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub repo_category: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub encrypted: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub enc_version: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub magic: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
    /// Per-library random salt of an `enc_version >= 3` library
    /// (`seaf_commit_to_data`: `if (enc_version >= 3) set "salt"`).
    ///
    /// Official clients check this on every commit they read:
    /// `commit_from_json_object()` returns NULL for `enc_version` 3/4 unless
    /// `salt` is present and exactly 64 hex chars, so a missing salt makes an
    /// encrypted library's history unreadable.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub salt: Option<String>,
    /// `pwd_hash`-based password verifier of a Seafile 11+ library
    /// (`seaf_commit_to_data`: `if (commit->pwd_hash) set "pwd_hash", …`).
    ///
    /// A library that carries one has **no** `magic`: upstream writes `magic`
    /// only when `!pwd_hash` (`common/commit-mgr.c`), and
    /// `commit_from_json_object()` falls back to `magic = pwd_hash` when the
    /// field is absent. Emitting both would make an official client verify the
    /// password against `magic`, which no longer exists.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pwd_hash: Option<String>,
    /// Algorithm of `pwd_hash`: `pbkdf2_sha256` or `argon2id`, compared verbatim
    /// by the clients.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pwd_hash_algo: Option<String>,
    /// Parameters of `pwd_hash_algo`. Omitted (rather than sent as JSON `null`
    /// like upstream's non-null-safe writer) when the server has no explicit
    /// value: the clients then fall back to the same defaults, which is what
    /// upstream's `NULL` achieves there.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pwd_hash_params: Option<String>,
    pub version: i32,
}
