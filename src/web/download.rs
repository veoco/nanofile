use axum::{
    extract::{Path, State},
    http::{HeaderMap, HeaderName, HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
};
use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};
use std::sync::Arc;

use crate::AppState;
use crate::auth::middleware::AuthUser;
use crate::entity::{fs_object, share_link};
use crate::error::AppError;
use crate::serialization::fs_json::FsFileData;
use crate::storage::download::Downloader;

/// GET /f/{token} — download via shared link token.
pub async fn shared_file_download(
    State(state): State<Arc<AppState>>,
    Path(token): Path<String>,
) -> Result<Response, AppError> {
    let link = share_link::Entity::find()
        .filter(share_link::Column::Token.eq(&token))
        .one(state.db.as_ref())
        .await?
        .ok_or_else(|| AppError::NotFound("link not found".into()))?;

    let content = Downloader::download_file(
        state.db.as_ref(),
        &link.repo_id,
        &link.path,
        &state.block_store,
    )
    .await
    .map_err(|_| AppError::NotFound("file not found".into()))?;

    Ok((
        StatusCode::OK,
        [(header::CONTENT_TYPE, "application/octet-stream")],
        content,
    )
        .into_response())
}

/// GET /repos/{repo_id}/files/{*path} — direct file download with auth.
pub async fn repo_file_download(
    _auth: AuthUser,
    State(state): State<Arc<AppState>>,
    Path((repo_id, path)): Path<(String, String)>,
) -> Result<Response, AppError> {
    let normalized = if path.starts_with('/') {
        path
    } else {
        format!("/{path}")
    };

    let content =
        Downloader::download_file(state.db.as_ref(), &repo_id, &normalized, &state.block_store)
            .await
            .map_err(|_| AppError::NotFound("file not found".into()))?;

    Ok((
        StatusCode::OK,
        [(header::CONTENT_TYPE, "application/octet-stream")],
        content,
    )
        .into_response())
}

/// GET /download-api/{token} — Token-authenticated file download.
///
/// Step B of the two-step download flow: the client first obtains a download
/// URL from `GET /api2/repos/{id}/file/?op=download`, then GETs this endpoint
/// to receive the raw file bytes.
pub async fn download_api(
    State(state): State<Arc<AppState>>,
    Path(token): Path<String>,
) -> Result<Response, AppError> {
    let info = state
        .token_manager
        .validate(&token)
        .ok_or_else(|| AppError::BadRequest("invalid or expired download token".into()))?;

    if info.op != "download" {
        return Err(AppError::BadRequest("token not valid for download".into()));
    }

    let repo_id = &info.repo_id;
    let path = &info.parent_dir;
    let filename = info.file_name.as_deref().unwrap_or("download");

    let content = Downloader::download_file(state.db.as_ref(), repo_id, path, &state.block_store)
        .await
        .map_err(|e| AppError::Internal(format!("download failed: {e}")))?;

    let content_len = content.len();
    let mut headers = HeaderMap::new();
    headers.insert(
        HeaderName::from_static("content-type"),
        HeaderValue::from_static("application/octet-stream"),
    );
    headers.insert(
        HeaderName::from_static("content-disposition"),
        HeaderValue::from_str(&format!("attachment; filename=\"{}\"", filename)).unwrap(),
    );
    headers.insert(
        HeaderName::from_static("content-length"),
        HeaderValue::from_str(&content_len.to_string()).unwrap(),
    );

    Ok((StatusCode::OK, headers, content).into_response())
}

/// `GET /blks/{token}/{file_id}/{block_id}`
///
/// Step B of the block download flow.  Validates the token, looks up the file
/// by `file_id`, verifies the block belongs to that file, and returns the raw
/// block bytes (matching seafile-server's `access_blks_cb`).
pub async fn block_download(
    State(state): State<Arc<AppState>>,
    Path((token, file_id, block_id)): Path<(String, String, String)>,
) -> Result<Response, AppError> {
    // Validate the downloadblks token.
    let info = state
        .token_manager
        .validate(&token)
        .ok_or_else(|| AppError::BadRequest("invalid or expired token".into()))?;

    if info.op != "downloadblks" {
        return Err(AppError::BadRequest(
            "token not valid for block download".into(),
        ));
    }

    let repo_id = &info.repo_id;

    // Look up the file by its fs_id in the fs_objects table.
    let file_obj = fs_object::Entity::find()
        .filter(fs_object::Column::RepoId.eq(repo_id))
        .filter(fs_object::Column::FsId.eq(&file_id))
        .one(state.db.as_ref())
        .await?
        .ok_or_else(|| AppError::NotFound("file not found".into()))?;

    // Parse the FsFileData to get the block list.
    let file_data: FsFileData = serde_json::from_str(&file_obj.data)
        .map_err(|e| AppError::Internal(format!("invalid file data: {e}")))?;

    // Verify the requested block_id belongs to this file.
    if !file_data.block_ids.contains(&block_id) {
        return Err(AppError::NotFound("block not found".into()));
    }

    // Read the block from the block store.
    let block_data = state
        .block_store
        .read_block(&block_id)
        .await
        .map_err(|_| AppError::NotFound("block data not found".into()))?;

    Ok((
        StatusCode::OK,
        [(header::CONTENT_TYPE, "application/octet-stream")],
        block_data,
    )
        .into_response())
}
