use sea_orm::{ConnectionTrait, DatabaseBackend, DatabaseConnection, Statement};

use crate::domain;
use base::common::{EMPTY_SHA1, FsDirData, FsFileData};
use base::error::AppError;

/// Compute fs_id, serialize, and INSERT OR IGNORE into fs_objects.
/// Returns the fs_id (or `EMPTY_SHA1` for empty directories).
pub async fn store_fs_dir_object(
    db: &DatabaseConnection,
    repo_id: &str,
    data: &FsDirData,
) -> Result<String, AppError> {
    if data.dirents.is_empty() {
        return Ok(EMPTY_SHA1.to_string());
    }
    let (fs_id, json) = domain::fs::compute_dir(data).expect("non-empty directory");
    let _ = db
        .execute(Statement::from_sql_and_values(
            DatabaseBackend::Sqlite,
            "INSERT OR IGNORE INTO fs_objects (repo_id, fs_id, obj_type, data) VALUES ($1, $2, $3, $4)",
            vec![
                repo_id.to_owned().into(),
                fs_id.clone().into(),
                (data.obj_type as i8).into(),
                json.into(),
            ],
        ))
        .await?;
    Ok(fs_id)
}

/// Compute fs_id, serialize, and INSERT OR IGNORE into fs_objects.
/// Returns the fs_id.
pub async fn store_fs_file_object(
    db: &DatabaseConnection,
    repo_id: &str,
    data: &FsFileData,
) -> Result<String, AppError> {
    let (fs_id, json) = domain::fs::compute_file(data);
    let _ = db
        .execute(Statement::from_sql_and_values(
            DatabaseBackend::Sqlite,
            "INSERT OR IGNORE INTO fs_objects (repo_id, fs_id, obj_type, data) VALUES ($1, $2, $3, $4)",
            vec![
                repo_id.to_owned().into(),
                fs_id.clone().into(),
                (data.obj_type as i8).into(),
                json.into(),
            ],
        ))
        .await?;
    Ok(fs_id)
}
