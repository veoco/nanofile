use crate::repository::Repositories;
use base::common::{FsDirData, FsFileData, SEAF_METADATA_TYPE_DIR};
use base::error::AppError;
use infra::common::EMPTY_SHA1;

/// Read and parse a directory fs_object (FsDirData) from the database.
pub async fn read_fs_dir_data(
    repos: &Repositories,
    repo_id: &str,
    fs_id: &str,
) -> Result<FsDirData, AppError> {
    // The zero hash (all zeros) is a sentinel in seafile's protocol,
    // used for empty/incomplete directories or when an fs_object
    // hasn't been fully committed yet. Treat it as an empty directory.
    if fs_id == EMPTY_SHA1 {
        return Ok(FsDirData {
            dirents: vec![],
            obj_type: SEAF_METADATA_TYPE_DIR,
            version: 1,
        });
    }
    let obj = repos
        .fs_object
        .find_by_repo_and_fs_id(repo_id, fs_id)
        .await?
        .ok_or_else(|| AppError::NotFound(format!("fs_object not found: {fs_id}")))?;
    let data: FsDirData =
        serde_json::from_str(&obj.data).map_err(|e| AppError::internal(e.to_string()))?;
    Ok(data)
}

/// Read and parse a file fs_object (FsFileData) from the database.
pub async fn read_fs_file_data(
    repos: &Repositories,
    repo_id: &str,
    fs_id: &str,
) -> Result<FsFileData, AppError> {
    let obj = repos
        .fs_object
        .find_by_repo_and_fs_id(repo_id, fs_id)
        .await?
        .ok_or_else(|| AppError::NotFound(format!("fs_object not found: {fs_id}")))?;
    let data: FsFileData =
        serde_json::from_str(&obj.data).map_err(|e| AppError::internal(e.to_string()))?;
    Ok(data)
}

/// Traverse the FS tree from root_fs_id following path segments,
/// returning the fs_id of the final segment.
///
/// Path should be absolute (e.g. `/dir/subdir/file.txt`).
/// Returns the root_fs_id itself if path is "/" or empty.
pub async fn resolve_fs_id(
    repos: &Repositories,
    repo_id: &str,
    root_fs_id: &str,
    path: &str,
) -> Result<String, AppError> {
    let segments: Vec<&str> = path
        .trim_start_matches('/')
        .split('/')
        .filter(|s| !s.is_empty())
        .collect();
    if segments.is_empty() {
        return Ok(root_fs_id.to_string());
    }

    let mut current_fs_id = root_fs_id.to_string();

    for segment in segments {
        let dir_data = read_fs_dir_data(repos, repo_id, &current_fs_id).await?;

        let entry = dir_data
            .dirents
            .iter()
            .find(|d| d.name == segment)
            .ok_or_else(|| AppError::NotFound(format!("path segment not found: {segment}")))?;

        current_fs_id = entry.id.clone();
    }

    Ok(current_fs_id)
}
