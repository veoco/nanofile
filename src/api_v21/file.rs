use axum::{
    Json,
    extract::{Path, Query, State},
    http::HeaderMap,
};
use sea_orm::{EntityTrait, QueryFilter};
use serde::Deserialize;
use std::sync::Arc;

use crate::AppState;
use crate::auth::middleware::AuthUser;
use crate::error::AppError;
use crate::serialization::fs_json::{DirEntryData, FsDirData, FsFileData, SEAF_METADATA_TYPE_DIR};
use crate::storage::file_ops::FileOps;

#[derive(Deserialize)]
pub struct CreateFileRequest {
    pub p: Option<String>,
}

/// POST /api/v2.1/repos/{repo_id}/file/
///
/// Creates an empty file placeholder in the FS tree.
pub async fn create_file_v21(
    auth: AuthUser,
    State(state): State<Arc<AppState>>,
    Path(repo_id): Path<String>,
    Json(req): Json<CreateFileRequest>,
) -> Result<Json<serde_json::Value>, AppError> {
    // Permission check
    crate::storage::check_repo_write_permission(state.db.as_ref(), &repo_id, auth.user_id).await?;

    let path = req
        .p
        .ok_or_else(|| AppError::BadRequest("path (p) required".into()))?;
    let path = if path.starts_with('/') {
        path
    } else {
        format!("/{}", path)
    };

    let file_name = path.rsplit_once('/').map(|(_, n)| n).unwrap_or(&path);
    let parent_path = match path.rsplit_once('/') {
        Some(("", _)) => "/",
        Some((parent, _)) => parent,
        None => "/",
    };

    if file_name.is_empty() {
        return Err(AppError::BadRequest("invalid path".into()));
    }

    let db = state.db.as_ref();

    // Create empty FsFileData
    let file_fs_data = FsFileData {
        block_ids: vec![],
        size: 0,
        obj_type: 1,
        version: 1,
    };
    let file_fs_id = file_fs_data.compute_and_store(db, &repo_id).await?;

    // Resolve parent directory (handles empty repo)
    let parent_fs_id = if parent_path == "/" {
        match get_head_root_id_no_err(db, &repo_id).await? {
            Some(root_id) => root_id,
            None => {
                // Empty repo — create root fs_object
                let empty_dir = FsDirData {
                    dirents: vec![],
                    obj_type: SEAF_METADATA_TYPE_DIR,
                    version: 1,
                };
                empty_dir.compute_and_store(db, &repo_id).await?
            }
        }
    } else {
        let head_root_id = get_head_root_id_no_err(db, &repo_id)
            .await?
            .ok_or_else(|| AppError::NotFound("repo has no commits".into()))?;
        crate::storage::resolve_fs_id(
            db,
            &repo_id,
            &head_root_id,
            parent_path,
            Some(state.path_cache.as_ref()),
        )
        .await
        .map_err(|e| AppError::Internal(format!("resolve parent failed: {e}")))?
    };

    // Add entry to parent's FsDirData and create commit
    FileOps::update_dir_tree_and_commit(
        db,
        &repo_id,
        parent_path,
        &parent_fs_id,
        &auth.email,
        &format!("Created empty file {}", file_name),
        Some(state.path_cache.as_ref()),
        |dirents| {
            if !dirents.iter().any(|d| d.name == file_name) {
                dirents.push(DirEntryData {
                    id: file_fs_id.clone(),
                    mode: crate::serialization::S_IFREG,
                    modifier: auth.email.clone(),
                    mtime: chrono::Utc::now().timestamp(),
                    name: file_name.to_string(),
                    size: 0,
                });
            }
            Ok(())
        },
    )
    .await
    .map_err(|e| AppError::Internal(e.to_string()))?;

    Ok(Json(serde_json::json!({"success": true})))
}

/// Like get_head_root_id but returns None instead of error on empty repo.
async fn get_head_root_id_no_err(
    db: &sea_orm::DatabaseConnection,
    repo_id: &str,
) -> Result<Option<String>, AppError> {
    use crate::entity::{commit, repo};
    use sea_orm::ColumnTrait as _;
    let repo_record = repo::Entity::find_by_id(repo_id)
        .one(db)
        .await?
        .ok_or_else(|| AppError::NotFound("Repository not found".to_string()))?;
    let head_commit_id = match repo_record.head_commit_id {
        Some(id) => id,
        None => return Ok(None),
    };
    let head = commit::Entity::find()
        .filter(commit::Column::RepoId.eq(repo_id))
        .filter(commit::Column::CommitId.eq(&head_commit_id))
        .one(db)
        .await?
        .ok_or_else(|| AppError::NotFound("Head commit not found".to_string()))?;
    Ok(Some(head.root_id))
}

/// GET /api/v2.1/repos/{repo_id}/file-uploaded-bytes/
///
/// Returns the number of bytes already uploaded for a resumable file upload.
/// Since nanofile handles uploads atomically (no partial upload state),
/// we always return `{"uploadedBytes": 0}` with an `Accept-Ranges: bytes`
/// header to signal that the client should send the entire file.
pub async fn file_uploaded_bytes(
    _auth: AuthUser,
    Path(_repo_id): Path<String>,
    Query(query): Query<UploadedBytesQuery>,
) -> Result<(HeaderMap, Json<serde_json::Value>), AppError> {
    if query.file_name.is_none() || query.file_name.as_deref() == Some("") {
        return Err(AppError::BadRequest("file_name invalid.".into()));
    }
    if query.parent_dir.is_none() || query.parent_dir.as_deref() == Some("") {
        return Err(AppError::BadRequest("parent_dir invalid.".into()));
    }

    let mut headers = HeaderMap::new();
    headers.insert("Accept-Ranges", "bytes".parse().unwrap());

    Ok((headers, Json(serde_json::json!({"uploadedBytes": 0}))))
}

#[derive(Deserialize)]
pub struct UploadedBytesQuery {
    pub file_name: Option<String>,
    pub parent_dir: Option<String>,
}

/// Wrapper for delete_dirent_v21 that works with Path(repo_id) instead of Path((repo_id, obj))
pub async fn delete_file_v21(
    auth: crate::auth::middleware::AuthUser,
    state: axum::extract::State<std::sync::Arc<crate::AppState>>,
    repo_id: axum::extract::Path<String>,
    query: axum::extract::Query<super::dir::V21DirQuery>,
) -> Result<axum::Json<serde_json::Value>, crate::error::AppError> {
    // Delegate to delete_dirent_v21 with obj="file"
    super::dir::delete_dirent_v21(
        auth,
        state,
        axum::extract::Path((repo_id.0, "file".to_string())),
        query,
    )
    .await
}
