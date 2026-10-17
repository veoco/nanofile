use sea_orm::{ConnectionTrait, DatabaseBackend, DatabaseConnection, Statement};

use crate::domain;
use base::common::{EMPTY_SHA1, FsDirData, FsFileData};
use base::error::AppError;

/// Store a directory object (`INSERT OR IGNORE`) and return its fs_id
/// (`EMPTY_SHA1` when empty). Entries are sorted into seafile's canonical
/// descending order first: the fs_id is the SHA-1 of the serialized object, so
/// any other order carries an id no client can reproduce. Every directory write
/// funnels through here, including insertion-order appends from REST and WebDAV.
pub async fn store_fs_dir_object(
    db: &DatabaseConnection,
    repo_id: &str,
    data: &FsDirData,
) -> Result<String, AppError> {
    if data.dirents.is_empty() {
        return Ok(EMPTY_SHA1.to_string());
    }
    let mut data = data.clone();
    domain::fs::sort_dirents_desc(&mut data.dirents);
    let (fs_id, json) = domain::fs::compute_dir(&data).expect("non-empty directory");
    let _ = db
        .execute_raw(Statement::from_sql_and_values(
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

/// Store a file object (`INSERT OR IGNORE`); a zero-byte file keeps the
/// `EMPTY_SHA1` sentinel (see `seaf_fs_manager_index_blocks()`), so nothing is
/// stored and every caller sees the id a client computes locally.
pub async fn store_fs_file_object(
    db: &DatabaseConnection,
    repo_id: &str,
    data: &FsFileData,
) -> Result<String, AppError> {
    let Some((fs_id, json)) = domain::fs::compute_file(data) else {
        return Ok(EMPTY_SHA1.to_string());
    };
    let _ = db
        .execute_raw(Statement::from_sql_and_values(
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
