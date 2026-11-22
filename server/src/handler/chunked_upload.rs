use axum::{
    Json, Router,
    extract::{Query, State},
    http::HeaderMap,
};
use serde::Deserialize;
use std::collections::HashMap;
use std::sync::Arc;

use crate::AppState;
use crate::middleware::repo_extractor::RepoPathWrite;
use base::error::AppError;

#[derive(Deserialize)]
pub struct BlksLinkQuery {
    pub p: Option<String>,
}

pub fn chunked_upload_routes() -> Router<Arc<AppState>> {
    Router::new()
        .route(
            "/{repo_id}/upload-blks-link/",
            axum::routing::get(upload_blks_link).post(upload_blks_link_post),
        )
        .route(
            "/{repo_id}/update-blks-link/",
            axum::routing::get(update_blks_link),
        )
}

pub async fn upload_blks_link(
    access: RepoPathWrite,
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(query): Query<BlksLinkQuery>,
) -> Result<Json<String>, AppError> {
    // Mints a block-upload token on a GET; see `repo_tokens` in `repos.rs`.
    crate::middleware::require_csrf_for_cookie_session(&headers, &state.csrf_secret)?;

    let parent_dir = query.p.as_deref().unwrap_or("/");
    let repo_id = &access.repo_id;
    ensure_not_encrypted(&state, repo_id).await?;

    let token = state.token_manager.generate(
        repo_id,
        access.user.user_id,
        &access.user.email,
        "upload-blks",
        parent_dir,
    );

    let url = build_blks_op_url(&state, "upload-blks-api", &token, &headers);

    Ok(Json(url))
}

/// `POST /api2/repos/{repo_id}/upload-blks-link/` — the mobile clients' first
/// call for a block upload (iOS `SeafUploadOperation.uploadLargeFileByBlocks`).
///
/// The body is `application/x-www-form-urlencoded` with `blklist`, a
/// comma-separated list of the client's block ids. The answer names the blocks
/// this server is missing and hands out the raw-block URL and the commit URL,
/// both bound to the same short-lived token.
pub async fn upload_blks_link_post(
    access: RepoPathWrite,
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(query): Query<BlksLinkQuery>,
    body: axum::body::Body,
) -> Result<Json<serde_json::Value>, AppError> {
    // Mints capability URLs for a multi-request upload, so a cookie session
    // needs the same CSRF guard as the GET link; bearer callers are exempt.
    crate::middleware::require_csrf_for_cookie_session(&headers, &state.csrf_secret)?;

    let parent_dir = query.p.as_deref().unwrap_or("/");
    let repo_id = &access.repo_id;
    ensure_not_encrypted(&state, repo_id).await?;

    let bytes =
        crate::handler::read_body_limited(body, crate::handler::MAX_SMALL_BODY_BYTES).await?;
    let form: HashMap<String, String> = serde_urlencoded::from_bytes(&bytes)
        .map_err(|_| AppError::BadRequest("invalid form data".into()))?;
    let requested: Vec<String> = form
        .get("blklist")
        .map(|list| {
            list.split(',')
                .map(str::trim)
                .filter(|id| !id.is_empty())
                .map(String::from)
                .collect()
        })
        .unwrap_or_default();

    // These ids are replayed into an fs object at commit time; reject a
    // malformed one before it reaches the store or that list.
    if let Some(bad) = requested
        .iter()
        .find(|id| !crate::handler::is_valid_block_id(id))
    {
        return Err(AppError::BadRequest(format!("invalid block id: {bad}")));
    }

    let missing =
        crate::handler::block::missing_blocks(state.block_store.clone(), repo_id, requested).await;

    let token = state.token_manager.generate(
        repo_id,
        access.user.user_id,
        &access.user.email,
        "upload-blks",
        parent_dir,
    );
    let raw_url = build_blks_op_url(&state, "upload-raw-blks-api", &token, &headers);
    let commit_url = format!(
        "{}?commitonly=true&ret-json=true",
        build_blks_op_url(&state, "upload-blks-api", &token, &headers)
    );

    Ok(Json(serde_json::json!({
        "rawblksurl": raw_url,
        "commiturl": commit_url,
        "blklist": missing,
    })))
}

pub async fn update_blks_link(
    access: RepoPathWrite,
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(query): Query<BlksLinkQuery>,
) -> Result<Json<String>, AppError> {
    crate::middleware::require_csrf_for_cookie_session(&headers, &state.csrf_secret)?;

    let parent_dir = query.p.as_deref().unwrap_or("/");
    let repo_id = &access.repo_id;
    ensure_not_encrypted(&state, repo_id).await?;

    let token = state.token_manager.generate(
        repo_id,
        access.user.user_id,
        &access.user.email,
        "update-blks",
        parent_dir,
    );

    // Upstream seahub mints `/update-blks-api/{token}` for this link
    // (`UpdateBlksLinkView`), and the write target comes from the request's
    // `target_file`, not from this token. Keep the path shape compatible; the
    // shared block handler branches on the `update-blks` op for update semantics.
    let url = build_blks_op_url(&state, "update-blks-api", &token, &headers);

    Ok(Json(url))
}

/// Refuse to hand out a block-upload URL for an encrypted library.
///
/// The block API is id-addressed: the client names each block (`sha1` of the bytes
/// it sends) and later commits by replaying that list, so the server cannot re-key
/// the blocks to `sha1(ciphertext)` without breaking that commit. Refusing here
/// fails close to the cause instead of at the first block write.
async fn ensure_not_encrypted(state: &AppState, repo_id: &str) -> Result<(), AppError> {
    if let Some(repo_model) = state.repos.repo.find_by_id(repo_id).await?
        && repo_model.encrypted != 0
    {
        return Err(AppError::BadRequest(
            "block upload is not supported for encrypted libraries".into(),
        ));
    }
    Ok(())
}

/// Build a block-upload URL on the Host-aware external base, so a LAN client that
/// reached the server by address is not told to upload to loopback.
fn build_blks_op_url(state: &AppState, op: &str, token: &str, headers: &HeaderMap) -> String {
    let host = headers.get("host").and_then(|v| v.to_str().ok());
    let base = state.config().server.download_url_base(host);
    format!("{}/{}/{}", base.trim_end_matches('/'), op, token)
}
