use sea_orm::DatabaseConnection;

use crate::error::AppError;
use crate::serialization::fs_json::{FsDirData, FsFileData};

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
