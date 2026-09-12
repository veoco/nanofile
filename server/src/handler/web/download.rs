use axum::{
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode, header},
    response::{IntoResponse, Response},
};
use serde::Deserialize;
use std::sync::Arc;

use crate::AppState;
use crate::fs::core::download::Downloader;
use crate::middleware::auth::AuthUser;
use base::common::FsFileData;
use base::error::AppError;

/// Reject a capability token whose owner is no longer an active account.
///
/// The `/download-api/{token}`, `/blks/{token}/…` and `/upload-api/{token}`
/// endpoints authenticate with an in-memory token rather than `AuthUser`, so
/// they never saw the `is_active` check every other surface applies. A
/// deactivated account kept read (and write) access for the token's remaining
/// TTL — exactly the window an administrator deactivates an account to close.
pub(crate) async fn ensure_token_user_active(
    state: &AppState,
    user_id: i32,
) -> Result<(), AppError> {
    let user = state
        .repos
        .user
        .find_by_id(user_id)
        .await?
        .ok_or(AppError::Unauthorized)?;
    if !user.is_active {
        return Err(AppError::Forbidden);
    }
    Ok(())
}

/// Helper: get decryption key for an encrypted repo if password is set.
///
/// Returns `None` if the repo is not encrypted, or `Some(Some((key, iv)))` if
/// the password is cached, or `RepoPasswdRequired` error if the repo is
/// encrypted but no password has been set.
pub(crate) async fn get_decryption_key_for_repo(
    state: &AppState,
    repo_id: &str,
    user_id: i32,
) -> Result<Option<(Vec<u8>, Vec<u8>)>, AppError> {
    ensure_token_user_active(state, user_id).await?;

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

/// Resolve the block cipher key for a **write** into `repo_id` made by
/// `user_id`, or reject the write before any block is stored.
///
/// Symmetric with [`get_decryption_key_for_repo`] (AES-256-CBC uses the same
/// key/IV in both directions). An encrypted library's key lives only in the
/// server-side password cache, which the official clients warm through
/// `POST /api2/repos/{id}/?op=setpassword` / `.../set-password/` before
/// uploading — iOS refreshes it every 300 s (`SeafUploadOperation.m:109-118`),
/// well inside this server's 3600 s cache TTL. Writing without it would store
/// plaintext in a library whose owner expects ciphertext, so a cold cache is a
/// hard error: `RepoPasswdRequired` (440) is exactly the status Android and iOS
/// map to "Library password is needed" and then retry after re-warming.
///
/// `anonymous_link` marks a request authenticated only by a shareable upload
/// link. Such a caller has no `user_id` whose password cache could hold the
/// key, so an encrypted library is rejected outright — matching the official
/// clients, which hide the upload-link action for encrypted libraries
/// (`seafile-client/src/filebrowser/file-table.cpp:386`,
/// `seadroid/.../BottomSheetMenuManager.java:365`).
pub(crate) async fn upload_block_key(
    state: &AppState,
    repo_id: &str,
    user_id: i32,
    anonymous_link: bool,
) -> Result<Option<(Vec<u8>, Vec<u8>)>, AppError> {
    // Every upload path funnels through here, so this is also where the
    // token-only paths get the `is_active` check the rest of the server
    // applies. Without it a deactivated account could keep writing until its
    // access token expired.
    ensure_token_user_active(state, user_id).await?;

    // The server-wide kill switch has to reach the token consumers too: with
    // anonymous links disabled, an upload URL that was minted (or is still in
    // its one-hour TTL) before the switch was flipped must stop accepting
    // writes. `ensure_share_links_enabled` only guarded the pages and the
    // creation endpoints.
    if anonymous_link && !state.config.server.share_link_enabled {
        return Err(AppError::Forbidden);
    }

    let repo_model = state
        .repos
        .repo
        .find_by_id(repo_id)
        .await?
        .ok_or_else(|| AppError::NotFound("repo not found".into()))?;

    if repo_model.encrypted == 0 {
        return Ok(None);
    }

    if anonymous_link {
        return Err(AppError::BadRequest(
            "cannot upload to an encrypted library through an upload link".into(),
        ));
    }

    get_decryption_key_for_repo(state, repo_id, user_id).await
}

#[derive(Deserialize)]
pub struct RepoFileQuery {
    /// `?dl=1` forces `Content-Disposition: attachment` (download).
    pub dl: Option<String>,
}

/// GET /repos/{repo_id}/files/{*path} — the unified web file-content endpoint.
///
/// Serves the raw bytes with Range support and a strong ETag, so browsers can
/// inline images/video, download with `?dl=1`, and revalidate with
/// `If-None-Match` (304) instead of re-downloading large originals.
pub async fn repo_file_download(
    auth: AuthUser,
    State(state): State<Arc<AppState>>,
    Path((repo_id, path)): Path<(String, String)>,
    Query(query): Query<RepoFileQuery>,
    headers: HeaderMap,
) -> Result<Response, AppError> {
    // Normalize through the shared sanitizer exactly like the sibling
    // `{*path}` handlers (`ui/files.rs`, the share view, WebDAV) instead of
    // concatenating a leading slash. Resolution below matches dirent names
    // literally, so a malformed path such as `..` or a NUL byte can only ever
    // 404 today — but a raw capture that skips validation is the kind of
    // sibling-path divergence that turns into path confusion later.
    let normalized = base::sanitize::safe_normalize_path(&path)
        .map_err(|e| AppError::BadRequest(format!("Invalid path: {e}")))?;

    // Check read permission (matching seahub's check_folder_permission behavior).
    crate::domain::permission::check_repo_read_permission(
        state.repos.member.as_ref(),
        &repo_id,
        auth.user_id,
    )
    .await?;

    // Check if repo is encrypted and if password is set
    let dec_key = get_decryption_key_for_repo(&state, &repo_id, auth.user_id).await?;

    let (file_data, block_ids) = Downloader::resolve_blocks(&state.repos, &repo_id, &normalized)
        .await
        .map_err(|_| AppError::NotFound("file not found".into()))?;

    let file_name = normalized
        .rsplit('/')
        .next()
        .unwrap_or("download")
        .to_string();

    // Strong ETag: the block IDs are content-addressed, so identical content
    // always yields the same validator and any edit changes it. Hash the ids
    // incrementally instead of joining them into one large intermediate string
    // (a file with tens of thousands of blocks would otherwise allocate a
    // multi-hundred-KB buffer just to build the validator).
    use sha1::Digest;
    let mut hasher = sha1::Sha1::new();
    for id in &block_ids {
        hasher.update(id.as_bytes());
        hasher.update(b"|");
    }
    let etag = format!("\"{}\"", hex::encode(hasher.finalize()));

    let range_header = headers.get(header::RANGE).and_then(|v| v.to_str().ok());

    // Conditional request: a matching validator with no Range → 304, skipping
    // the block reads entirely. Range requests must still stream their slice.
    let if_none_match = headers
        .get(header::IF_NONE_MATCH)
        .and_then(|v| v.to_str().ok());
    let matches = match if_none_match {
        Some("*") => true,
        Some(v) => v.split(',').any(|t| t.trim() == etag),
        None => false,
    };
    if matches && range_header.is_none() {
        return Ok(StatusCode::NOT_MODIFIED.into_response());
    }

    let content_disposition = if query.dl.as_deref() == Some("1") {
        Some(crate::fs::core::download::content_disposition(
            &file_name, true,
        ))
    } else {
        None
    };

    Ok(crate::fs::core::download::file_download_response(
        crate::fs::core::download::FileDownloadParams {
            repo_id: repo_id.clone(),
            block_ids,
            block_store: state.block_store.clone(),
            enc_key: dec_key,
            total_size: file_data.size.max(0) as u64,
            content_type: crate::ui::files::mime_guess(&file_name),
            content_disposition,
            range_header: range_header.map(|s| s.to_string()),
            etag: Some(etag),
        },
    ))
}

/// GET /download-api/{token} — Token-authenticated file download.
///
/// Step B of the two-step download flow: the client first obtains a download
/// URL from `GET /api2/repos/{id}/file/?op=download`, then GETs this endpoint
/// to receive the raw file bytes.
pub async fn download_api(
    State(state): State<Arc<AppState>>,
    Path(token): Path<String>,
    headers: HeaderMap,
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

    let (file_data, block_ids) = Downloader::resolve_blocks(&state.repos, &repo_id, &path)
        .await
        .map_err(|e| AppError::Internal(format!("download failed: {e}")))?;

    let disposition = crate::fs::core::download::content_disposition(&filename, true);
    let range_header = headers.get(header::RANGE).and_then(|v| v.to_str().ok());
    Ok(crate::fs::core::download::file_download_response(
        crate::fs::core::download::FileDownloadParams {
            repo_id: repo_id.clone(),
            block_ids,
            block_store: state.block_store.clone(),
            enc_key: dec_key,
            total_size: file_data.size.max(0) as u64,
            content_type: "application/octet-stream",
            content_disposition: Some(disposition),
            range_header: range_header.map(|s| s.to_string()),
            etag: None,
        },
    ))
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

    // Re-check read permission: the token outlives membership, so a user
    // removed from the repo must not keep reading blocks.
    crate::domain::permission::check_repo_read_permission(
        state.repos.member.as_ref(),
        repo_id,
        info.user_id,
    )
    .await?;

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
        .read_block(repo_id, &block_id)
        .await
        .map_err(|_| AppError::NotFound("block data not found".into()))?;

    Ok((
        StatusCode::OK,
        [(header::CONTENT_TYPE, "application/octet-stream")],
        block_data,
    )
        .into_response())
}
