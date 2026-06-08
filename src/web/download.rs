use axum::{
    extract::{Path, State},
    http::{StatusCode, header},
    response::{IntoResponse, Response},
};
use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};
use std::sync::Arc;

use crate::AppState;
use crate::auth::middleware::AuthUser;
use crate::entity::share_link;
use crate::error::AppError;
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
