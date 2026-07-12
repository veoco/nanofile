use sea_orm::DatabaseConnection;

use crate::domain;
use crate::error::AppError;
use base::common::{FsDirData, FsFileData};

/// Serialize an FsFileData to JSON, compute its SHA1 ID, check the DB,
/// and insert if the object does not already exist. Returns the fs_id.
pub async fn store_fs_file_object(
    db: &DatabaseConnection,
    repo_id: &str,
    file_data: FsFileData,
) -> Result<String, AppError> {
    domain::fs::store_file_data(db, repo_id, &file_data).await
}

/// Serialize an FsDirData to JSON, compute its SHA1 ID, check the DB,
/// and insert if the object does not already exist. Returns the fs_id.
pub async fn store_fs_dir_object(
    db: &DatabaseConnection,
    repo_id: &str,
    dir_data: FsDirData,
) -> Result<String, AppError> {
    domain::fs::store_dir_data(db, repo_id, &dir_data).await
}
