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
/// endpoints authenticate with an in-memory token rather than `AuthUser`, so they
/// never saw the `is_active` check every other surface applies — a deactivated
/// account kept access for the token's remaining TTL.
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

/// Decryption key for an encrypted repo: `None` when unencrypted, the cached
/// `(key, iv)` when the password is set, else `RepoPasswdRequired`.
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

/// Resolve the block cipher key for a **write** into `repo_id` by `user_id`, or
/// reject the write before any block is stored.
///
/// Symmetric with [`get_decryption_key_for_repo`] (AES-256-CBC uses the same
/// key/IV both ways). An encrypted library's key lives only in the server-side
/// password cache that the official clients warm via `?op=setpassword` before
/// uploading (iOS refreshes every 300 s, inside this server's 3600 s TTL), so a
/// cold cache is a hard error: `RepoPasswdRequired` (440) is what Android and iOS
/// map to "Library password is needed". Writing without it would store plaintext
/// in a library whose owner expects ciphertext.
///
/// `anonymous_link` marks a request authenticated only by a shareable upload
/// link: no `user_id`'s cache can hold the key, so an encrypted library is
/// refused outright, matching the official clients that hide upload-link actions
/// for encrypted libraries.
pub(crate) async fn upload_block_key(
    state: &AppState,
    repo_id: &str,
    user_id: i32,
    anonymous_link: bool,
) -> Result<Option<(Vec<u8>, Vec<u8>)>, AppError> {
    // Every upload path funnels through here, so this is also where the token-only
    // paths get the `is_active` check the rest of the server applies.
    ensure_token_user_active(state, user_id).await?;

    // The server-wide kill switch has to reach the token consumers too: with
    // anonymous links disabled, an already-minted upload URL (still within its
    // TTL) must stop accepting writes, not just stop being created.
    if anonymous_link && !state.config().server.share_link_enabled {
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
    /// `?dl=1` forces `Content-Disposition: attachment` (download); nanofile's
    /// own web UI uses it.
    pub dl: Option<String>,
    /// `view` (inline) or `download` (attachment), matching seahub's
    /// `FILE_SERVER_ROOT/repos/{id}/files/{path}?op=…` contract. A missing `op`
    /// keeps the old inline-by-default behaviour.
    pub op: Option<String>,
}

/// GET /repos/{repo_id}/files/{*path} — the unified web file-content endpoint.
///
/// Serves raw bytes with Range support and a strong ETag, so browsers can inline
/// media, download with `?dl=1`, and revalidate with `If-None-Match` (304).
pub async fn repo_file_download(
    auth: AuthUser,
    State(state): State<Arc<AppState>>,
    Path((repo_id, path)): Path<(String, String)>,
    Query(query): Query<RepoFileQuery>,
    headers: HeaderMap,
) -> Result<Response, AppError> {
    // Normalize through the shared sanitizer like the sibling `{*path}` handlers
    // instead of concatenating a leading slash: a raw capture that skips
    // validation is how sibling-path divergence turns into path confusion.
    let normalized = base::sanitize::safe_normalize_path(&path)
        .map_err(|e| AppError::BadRequest(format!("Invalid path: {e}")))?;

    crate::domain::permission::check_repo_read_permission(
        state.repos.member.as_ref(),
        &repo_id,
        auth.user_id,
    )
    .await?;

    let dec_key = get_decryption_key_for_repo(&state, &repo_id, auth.user_id).await?;

    let (file_data, block_ids) = Downloader::resolve_blocks(&state.repos, &repo_id, &normalized)
        .await
        .map_err(|_| AppError::NotFound("file not found".into()))?;

    let file_name = normalized
        .rsplit('/')
        .next()
        .unwrap_or("download")
        .to_string();

    // Strong ETag: the block ids are content-addressed, so identical content
    // always yields the same validator and any edit changes it. Hash them
    // incrementally rather than joining them into one large string.
    use sha1::Digest;
    let mut hasher = sha1::Sha1::new();
    for id in &block_ids {
        hasher.update(id.as_bytes());
        hasher.update(b"|");
    }
    let etag = format!("\"{}\"", hex::encode(hasher.finalize()));

    let range_header = headers.get(header::RANGE).and_then(|v| v.to_str().ok());

    // A matching validator with no Range means 304, skipping the block reads.
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

    // seahub's `accessV2CB` distinguishes `op=view` (inline) from `op=download`
    // (attachment), and the official frontend's download button sends the latter.
    // `?dl=1` is nanofile's older alias; an absent `op` lets the browser decide.
    let content_disposition = match query.op.as_deref() {
        None | Some("") => {
            if query.dl.as_deref() == Some("1") {
                Some(crate::fs::core::download::content_disposition(
                    &file_name, true,
                ))
            } else {
                None
            }
        }
        Some("download") => Some(crate::fs::core::download::content_disposition(
            &file_name, true,
        )),
        Some("view") => Some(crate::fs::core::download::content_disposition(
            &file_name, false,
        )),
        Some(other) => {
            return Err(AppError::BadRequest(format!(
                "Operation is neither view nor download: {other}"
            )));
        }
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

/// GET /download-api/{token} — token-authenticated file download, step B of the
/// two-step flow (the client first obtains a download URL, then GETs this).
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

    let dec_key = get_decryption_key_for_repo(&state, &repo_id, info.user_id).await?;

    let (file_data, block_ids) = Downloader::resolve_blocks(&state.repos, &repo_id, &path)
        .await
        .map_err(|e| AppError::Internal(format!("download failed: {e}")))?;

    let disposition = crate::fs::core::download::content_disposition(&filename, true);
    let range_header = headers.get(header::RANGE).and_then(|v| v.to_str().ok());
    // Canonical `/files/{token}/{name}` sets a validator from the object id, so
    // repeat previews/downloads can revalidate instead of re-streaming.
    let etag = crate::fs::core::download::blocks_etag(&block_ids);
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
            etag: Some(etag),
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
