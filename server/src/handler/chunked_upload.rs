use axum::{
    Json, Router,
    extract::{Query, State},
    http::HeaderMap,
};
use serde::Deserialize;
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
            axum::routing::get(upload_blks_link),
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

    let url = build_blks_op_url(&state, "upload-blks-api", &token);

    Ok(Json(url))
}

pub async fn update_blks_link(
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
        "update-blks",
        parent_dir,
    );

    let url = build_blks_op_url(&state, "update-blks-api", &token);

    Ok(Json(url))
}

/// Refuse to hand out a block-upload URL for an encrypted library.
///
/// The block API is id-addressed: the client names each block (`sha1` of the
/// bytes it sends) and later commits by replaying that list, so the server
/// cannot re-key the blocks to `sha1(ciphertext)` without breaking the commit
/// the client is about to make. Refusing here keeps the refusal close to the
/// cause instead of failing at the first block write.
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

fn build_blks_op_url(state: &AppState, op: &str, token: &str) -> String {
    let base = state.config.server.site_url_origin();
    format!("{}/{}/{}", base.trim_end_matches('/'), op, token)
}
