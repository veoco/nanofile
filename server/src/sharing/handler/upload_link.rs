use axum::{
    Json, Router,
    extract::{Path, State},
};
use serde::Deserialize;
use std::sync::Arc;

use crate::AppState;
use crate::auth::middleware::AuthUser;
use crate::error::AppError;
use crate::sharing::service::link;

#[derive(Deserialize)]
pub struct CreateUploadLinkRequest {
    pub repo_id: String,
    pub path: String,
    pub password: Option<String>,
    pub expires_at: Option<i64>,
}

pub fn upload_link_routes() -> Router<Arc<AppState>> {
    Router::new()
        .route(
            "/",
            axum::routing::get(list_upload_links).post(create_upload_link),
        )
        .route("/{token}", axum::routing::delete(delete_upload_link))
}

pub async fn list_upload_links(
    auth: AuthUser,
    State(state): State<Arc<AppState>>,
) -> Result<Json<Vec<link::UploadLinkInfo>>, AppError> {
    let infos = link::list_upload_links(&state.repos, auth.user_id).await?;
    Ok(Json(infos))
}

pub async fn create_upload_link(
    auth: AuthUser,
    State(state): State<Arc<AppState>>,
    Json(req): Json<CreateUploadLinkRequest>,
) -> Result<Json<link::UploadLinkInfo>, AppError> {
    let info = link::create_upload_link(
        state.db.as_ref(),
        &state.config,
        &req.repo_id,
        &req.path,
        req.password.as_deref(),
        req.expires_at,
        auth.user_id,
    )
    .await?;
    Ok(Json(info))
}

pub async fn delete_upload_link(
    auth: AuthUser,
    State(state): State<Arc<AppState>>,
    Path(token): Path<String>,
) -> Result<(), AppError> {
    link::delete_upload_link(&state.repos, &token, auth.user_id).await?;
    Ok(())
}
