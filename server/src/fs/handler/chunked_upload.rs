use axum::{
    Json, Router,
    extract::{Query, State},
};
use serde::Deserialize;
use std::sync::Arc;

use crate::AppState;
use crate::auth::RepoPathWrite;
use crate::error::AppError;

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
    Query(query): Query<BlksLinkQuery>,
) -> Result<Json<String>, AppError> {
    let parent_dir = query.p.as_deref().unwrap_or("/");
    let repo_id = &access.repo_id;

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
    Query(query): Query<BlksLinkQuery>,
) -> Result<Json<String>, AppError> {
    let parent_dir = query.p.as_deref().unwrap_or("/");
    let repo_id = &access.repo_id;

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

fn build_blks_op_url(state: &AppState, op: &str, token: &str) -> String {
    let base = state.config.server.site_url_origin();
    format!("{}/{}/{}", base.trim_end_matches('/'), op, token)
}
