pub use crate::common::constants::{SEAF_METADATA_TYPE_DIR, SEAF_METADATA_TYPE_FILE};

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

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct FsDirData {
    pub dirents: Vec<DirEntryData>,
    #[serde(rename = "type")]
    pub obj_type: i32,
    pub version: i32,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct FsFileData {
    pub block_ids: Vec<String>,
    pub size: i64,
    #[serde(rename = "type")]
    pub obj_type: i32,
    pub version: i32,
}

impl FsDirData {
    pub fn to_compact_json(&self) -> String {
        let obj = serde_json::json!({
            "dirents": self.dirents,
            "type": self.obj_type,
            "version": self.version,
        });
        obj.to_string()
    }
}

impl FsFileData {
    pub fn to_compact_json(&self) -> String {
        let obj = serde_json::json!({
            "block_ids": self.block_ids,
            "size": self.size,
            "type": self.obj_type,
            "version": self.version,
        });
        obj.to_string()
    }
}
