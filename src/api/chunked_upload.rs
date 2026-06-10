use axum::{
    Json, Router,
    extract::{Path, Query, State},
};
use serde::Deserialize;
use std::sync::Arc;

use crate::AppState;
use crate::auth::middleware::AuthUser;
use crate::error::AppError;

// ─── Query types ─────────────────────────────────────────────────────────────

#[derive(Deserialize)]
pub struct BlksLinkQuery {
    /// Parent directory path (optional, defaults to "/").
    pub p: Option<String>,
}

// ─── Routes ──────────────────────────────────────────────────────────────────

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

// ─── Handlers ────────────────────────────────────────────────────────────────

/// GET /api2/repos/{repo_id}/upload-blks-link/
///
/// Returns a JSON string URL for the block upload API endpoint.
/// Response: `"http://host:port/upload-blks-api/{token}"`
///
/// This matches the seahub endpoint at `/tmp/seahub/tests/api/test_files.rs`
/// which returns a quoted URL string.
pub async fn upload_blks_link(
    auth: AuthUser,
    State(state): State<Arc<AppState>>,
    Path(repo_id): Path<String>,
    Query(query): Query<BlksLinkQuery>,
) -> Result<Json<String>, AppError> {
    let parent_dir = query.p.as_deref().unwrap_or("/");

    // Permission check (also validates repo exists)
    crate::storage::check_repo_write_permission(state.db.as_ref(), &repo_id, auth.user_id).await?;

    let token = state.token_manager.generate(
        &repo_id,
        auth.user_id,
        &auth.email,
        "upload-blks",
        parent_dir,
    );

    let url = build_blks_op_url(&state, "upload-blks-api", &token);

    Ok(Json(url))
}

/// GET /api2/repos/{repo_id}/update-blks-link/
///
/// Same as upload_blks_link but for update (overwrite) operations.
/// Returns a JSON string URL for the block update API endpoint.
pub async fn update_blks_link(
    auth: AuthUser,
    State(state): State<Arc<AppState>>,
    Path(repo_id): Path<String>,
    Query(query): Query<BlksLinkQuery>,
) -> Result<Json<String>, AppError> {
    let parent_dir = query.p.as_deref().unwrap_or("/");

    crate::storage::check_repo_write_permission(state.db.as_ref(), &repo_id, auth.user_id).await?;

    let token = state.token_manager.generate(
        &repo_id,
        auth.user_id,
        &auth.email,
        "update-blks",
        parent_dir,
    );

    let url = build_blks_op_url(&state, "update-blks-api", &token);

    Ok(Json(url))
}

// ─── Helpers ─────────────────────────────────────────────────────────────────

/// Build a server URL for a block upload operation.
fn build_blks_op_url(state: &AppState, op: &str, token: &str) -> String {
    let host = if state.config.server.addr == "0.0.0.0"
        || state.config.server.addr == "::"
        || state.config.server.addr == "127.0.0.1"
    {
        "127.0.0.1"
    } else {
        &state.config.server.addr
    };
    format!(
        "http://{}:{}/{}/{}",
        host, state.config.server.port, op, token
    )
}
