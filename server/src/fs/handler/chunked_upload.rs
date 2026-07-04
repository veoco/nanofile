use axum::{
    Json, Router,
    extract::{Path, Query, State},
};
use serde::Deserialize;
use std::sync::Arc;

use crate::AppState;
use crate::auth::middleware::AuthUser;
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
        "upload-blks",
        parent_dir,
    );

    let url = build_blks_op_url(&state, "upload-blks-api", &token);

    Ok(Json(url))
}

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

fn build_blks_op_url(state: &AppState, op: &str, token: &str) -> String {
    let base = state.config.server.site_url_origin();
    format!("{}/{}/{}", base.trim_end_matches('/'), op, token)
}
