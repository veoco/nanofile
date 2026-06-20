use sea_orm::{ConnectionTrait, DatabaseBackend, DatabaseConnection, Statement};

use crate::common::constants::EMPTY_SHA1;
use crate::error::AppError;
use crate::serialization::fs_json::{
    FsDirData, FsFileData, SEAF_METADATA_TYPE_DIR, SEAF_METADATA_TYPE_FILE,
};

/// Serialize to compact JSON, compute SHA1, insert if new (skips
/// duplicate with INSERT OR IGNORE).  Returns the computed fs_id.
///
/// Matches seafile's write_seafile() flow:
///   seafile_to_json() → calculate SHA1 → check exists → write.
///
/// Unlike the original implementation, we skip the separate existence-check
/// SELECT and rely on the UNIQUE(repo_id, fs_id) constraint to silently
/// ignore duplicates.  This halves the query count for this hot path.
impl FsFileData {
    pub async fn compute_and_store(
        self,
        db: &DatabaseConnection,
        repo_id: &str,
    ) -> Result<String, AppError> {
        let json = self.to_compact_json();
        let fs_id = crate::crypto::fs_id::sha1_hex(json.as_bytes());

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

impl FsDirData {
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
        let fs_id = crate::crypto::fs_id::sha1_hex(json.as_bytes());

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

/// Serialize an FsFileData to JSON, compute its SHA1 ID, check the DB,
/// and insert if the object does not already exist. Returns the fs_id.
///
/// Prefer calling `file_data.compute_and_store(db, repo_id).await`
/// directly instead.
pub async fn store_fs_file_object(
    db: &DatabaseConnection,
    repo_id: &str,
    file_data: FsFileData,
) -> Result<String, AppError> {
    file_data.compute_and_store(db, repo_id).await
}

/// Serialize an FsDirData to JSON, compute its SHA1 ID, check the DB,
/// and insert if the object does not already exist. Returns the fs_id.
///
/// Prefer calling `dir_data.compute_and_store(db, repo_id).await`
/// directly instead.
pub async fn store_fs_dir_object(
    db: &DatabaseConnection,
    repo_id: &str,
    dir_data: FsDirData,
) -> Result<String, AppError> {
    dir_data.compute_and_store(db, repo_id).await
}
