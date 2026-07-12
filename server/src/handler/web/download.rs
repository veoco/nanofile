use axum::{
    body::Body,
    extract::{Path, Query, State},
    http::{HeaderMap, HeaderName, HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
};
use futures::{Stream, StreamExt};
use std::collections::HashMap;
use std::sync::Arc;

use crate::AppState;
use crate::fs::core::download::Downloader;
use crate::middleware::auth::AuthUser;
use base::common::FsFileData;
use base::error::AppError;

/// Build a streaming body that reads and yields blocks one at a time.
///
/// `block_ids` — list of block SHA-1 hashes to stream.
/// `block_store` — content-addressed block storage backend.
/// `enc_key` — optional decryption key (None = plaintext blocks).
fn stream_blocks(
    block_ids: Vec<String>,
    block_store: infra::storage::DynBlockStorage,
    enc_key: Option<(Vec<u8>, Vec<u8>)>,
) -> impl Stream<Item = Result<bytes::Bytes, std::io::Error>> + 'static {
    futures::stream::iter(block_ids.into_iter().map(move |block_id| {
        let store = block_store.clone();
        let key = enc_key.clone();
        async move {
            let data = store
                .read_block(&block_id)
                .await
                .map_err(|e| std::io::Error::other(e.to_string()))?;
            let data = match &key {
                Some((k, iv)) => infra::crypto::random_key::decrypt_block(&data, k, iv)
                    .map_err(|e| std::io::Error::other(e.to_string()))?,
                None => data,
            };
            Ok(bytes::Bytes::from(data))
        }
    }))
    .buffered(4)
}

/// GET /f/{token} — download via shared link token.
///
/// Supports password-protected links via the `X-Seafile-Sharelink-Password`
/// HTTP header or `password` query parameter.
/// Returns 404 for expired links and 403 for wrong/missing passwords.
pub async fn shared_file_download(
    State(state): State<Arc<AppState>>,
    Path(token): Path<String>,
    headers: HeaderMap,
    Query(params): Query<HashMap<String, String>>,
) -> Result<Response, AppError> {
    let link = state
        .repos
        .share_link
        .find_by_token(&token)
        .await?
        .ok_or_else(|| AppError::NotFound("link not found".into()))?;

    // Check expiry
    if let Some(expires_at) = link.expires_at
        && chrono::Utc::now().timestamp() > expires_at
    {
        return Err(AppError::NotFound("link has expired".into()));
    }

    // Check password if set
    if let Some(ref stored_hash) = link.password {
        let provided = headers
            .get("X-Seafile-Sharelink-Password")
            .and_then(|v| v.to_str().ok().map(|s| s.to_string()))
            .or_else(|| params.get("password").cloned())
            .ok_or_else(|| AppError::BadRequest("password required".into()))?;

        if !crate::service::auth::password::verify_password(
            &provided,
            stored_hash,
            state.config.auth.password_hash_iterations,
        ) {
            return Err(AppError::Forbidden);
        }
    }

    let (_file_data, block_ids) =
        Downloader::resolve_blocks(&state.repos, state.db.as_ref(), &link.repo_id, &link.path)
            .await
            .map_err(|_| AppError::NotFound("file not found".into()))?;

    let stream = stream_blocks(block_ids, state.block_store.clone(), None);

    // Increment view_cnt asynchronously (fire-and-forget)
    let share_link_repo = state.repos.share_link.clone();
    let link_id = link.id;
    tokio::spawn(async move {
        let _ = share_link_repo.increment_view_cnt(link_id).await;
    });

    Ok((
        StatusCode::OK,
        [(header::CONTENT_TYPE, "application/octet-stream")],
        Body::from_stream(stream),
    )
        .into_response())
}

/// Helper: get decryption key for an encrypted repo if password is set.
///
/// Returns `None` if the repo is not encrypted, or `Some(Some((key, iv)))` if
/// the password is cached, or `RepoPasswdRequired` error if the repo is
/// encrypted but no password has been set.
async fn get_decryption_key_for_repo(
    state: &AppState,
    repo_id: &str,
    user_id: i32,
) -> Result<Option<(Vec<u8>, Vec<u8>)>, AppError> {
    let repo_model = state
        .repos
        .repo
        .find_by_id(repo_id)
        .await?
        .ok_or_else(|| AppError::NotFound("repo not found".into()))?;

    if repo_model.encrypted == 0 {
        return Ok(None); // Not encrypted, no key needed
    }

    if state
        .password_manager
        .is_password_set(repo_id, user_id)
        .await
    {
        let key = state
            .password_manager
            .get_decrypt_key(repo_id, user_id)
            .await;
        Ok(key)
    } else {
        Err(AppError::RepoPasswdRequired)
    }
}

/// GET /repos/{repo_id}/files/{*path} — direct file download with auth.
pub async fn repo_file_download(
    auth: AuthUser,
    State(state): State<Arc<AppState>>,
    Path((repo_id, path)): Path<(String, String)>,
) -> Result<Response, AppError> {
    let normalized = if path.starts_with('/') {
        path
    } else {
        format!("/{path}")
    };

    // Check read permission (matching seahub's check_folder_permission behavior).
    crate::domain::permission::check_repo_read_permission(
        state.repos.member.as_ref(),
        &repo_id,
        auth.user_id,
    )
    .await?;

    // Check if repo is encrypted and if password is set
    let dec_key = get_decryption_key_for_repo(&state, &repo_id, auth.user_id).await?;

    let (_file_data, block_ids) =
        Downloader::resolve_blocks(&state.repos, state.db.as_ref(), &repo_id, &normalized)
            .await
            .map_err(|_| AppError::NotFound("file not found".into()))?;

    let stream = stream_blocks(block_ids, state.block_store.clone(), dec_key);

    Ok((
        StatusCode::OK,
        [(header::CONTENT_TYPE, "application/octet-stream")],
        Body::from_stream(stream),
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

    // Re-check that user still has read permission on the repo.
    // Permissions may have been revoked between token issuance and use.
    crate::domain::permission::check_repo_read_permission(
        state.repos.member.as_ref(),
        &info.repo_id,
        info.user_id,
    )
    .await?;

    let repo_id = info.repo_id.clone();
    let path = info.parent_dir.clone();
    let filename = info.file_name.as_deref().unwrap_or("download").to_string();

    // Check if repo is encrypted and if password is set
    let dec_key = get_decryption_key_for_repo(&state, &repo_id, info.user_id).await?;

    let (_file_data, block_ids) =
        Downloader::resolve_blocks(&state.repos, state.db.as_ref(), &repo_id, &path)
            .await
            .map_err(|e| AppError::Internal(format!("download failed: {e}")))?;

    // We don't know the size upfront when streaming, so use
    // Transfer-Encoding: chunked (omit Content-Length).
    let stream = stream_blocks(block_ids, state.block_store.clone(), dec_key);

    let mut headers = HeaderMap::new();
    headers.insert(
        HeaderName::from_static("content-type"),
        HeaderValue::from_static("application/octet-stream"),
    );
    headers.insert(
        HeaderName::from_static("content-disposition"),
        HeaderValue::from_str(&format!("attachment; filename=\"{}\"", filename))
            .unwrap_or_else(|_| HeaderValue::from_static("attachment")),
    );

    Ok((StatusCode::OK, headers, Body::from_stream(stream)).into_response())
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
    let file_obj = state
        .repos
        .fs_object
        .find_by_repo_and_fs_id(repo_id, &file_id)
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
