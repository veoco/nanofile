use sea_orm::{ConnectionTrait, DatabaseBackend, DatabaseConnection, Statement};

use nanofile_domain::AppError;
use crate::common::EMPTY_SHA1;
use crate::crypto::fs_id::sha1_hex;

pub use crate::common::{SEAF_METADATA_TYPE_DIR, SEAF_METADATA_TYPE_FILE};

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

    /// Serialize to compact JSON, compute SHA1, insert if new (skips
    /// duplicate with INSERT OR IGNORE).  Returns the computed fs_id.
    ///
    /// Empty directories use the EMPTY_SHA1 sentinel and are never stored,
    /// matching seafile's seaf_dir_save() / seaf_dir_new() behavior:
    ///   seaf_dir_new: entries==NULL → dir_id = EMPTY_SHA1
    ///   seaf_dir_save: dir_id == EMPTY_SHA1 → skip
    pub async fn compute_and_store(
        self,
        db: &DatabaseConnection,
        repo_id: &str,
    ) -> Result<String, AppError> {
        // Empty dirs use the EMPTY_SHA1 sentinel per seafile convention.
        if self.dirents.is_empty() {
            return Ok(EMPTY_SHA1.to_string());
        }

        let json = self.to_compact_json();
        let fs_id = sha1_hex(json.as_bytes());

        let _ = db
            .execute(Statement::from_sql_and_values(
                DatabaseBackend::Sqlite,
                "INSERT OR IGNORE INTO fs_objects (repo_id, fs_id, obj_type, data) VALUES ($1, $2, $3, $4)",
                vec![
                    repo_id.to_owned().into(),
                    fs_id.clone().into(),
                    (SEAF_METADATA_TYPE_DIR as i8).into(),
                    json.into(),
                ],
            ))
            .await?;

        Ok(fs_id)
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

    /// Serialize to compact JSON, compute SHA1, insert if new (skips
    /// duplicate with INSERT OR IGNORE).  Returns the computed fs_id.
    ///
    /// Matches seafile's write_seafile() flow:
    ///   seafile_to_json() → calculate SHA1 → check exists → write.
    ///
    /// Unlike the original implementation, we skip the separate existence-check
    /// SELECT and rely on the UNIQUE(repo_id, fs_id) constraint to silently
    /// ignore duplicates.  This halves the query count for this hot path.
    pub async fn compute_and_store(
        self,
        db: &DatabaseConnection,
        repo_id: &str,
    ) -> Result<String, AppError> {
        let json = self.to_compact_json();
        let fs_id = sha1_hex(json.as_bytes());

        let _ = db
            .execute(Statement::from_sql_and_values(
                DatabaseBackend::Sqlite,
                "INSERT OR IGNORE INTO fs_objects (repo_id, fs_id, obj_type, data) VALUES ($1, $2, $3, $4)",
                vec![
                    repo_id.to_owned().into(),
                    fs_id.clone().into(),
                    (SEAF_METADATA_TYPE_FILE as i8).into(),
                    json.into(),
                ],
            ))
            .await?;

        Ok(fs_id)
    }
}
