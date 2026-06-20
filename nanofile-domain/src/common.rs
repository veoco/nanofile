//! Shared constants and types for the nanofile ecosystem.
//!
//! This module is split across crate boundaries:
//! - The actual code lives here in `nanofile-domain`
//! - `nanofile-infra` re-exports it and adds `util` functions
//! - `nanofile-server` accesses everything via `crate::common::*`

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
#[derive(serde::Serialize)]
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
